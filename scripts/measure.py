#!/usr/bin/env python3
"""Linux fixture-only idle/visibility/restart measurement. Never touches an mprime installation."""
import json, os, pathlib, platform, signal, sqlite3, subprocess, tempfile, time
root = pathlib.Path(__file__).resolve().parents[1]
binary = root / 'target/release/gimps-watch'
def wait_for(predicate, seconds=30):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate(): return
        time.sleep(.02)
    raise AssertionError('measurement timed out')
with tempfile.TemporaryDirectory() as directory:
    d = pathlib.Path(directory)
    log, db, config = d/'worker.log', d/'state.sqlite', d/'watch.toml'
    records = '[Worker #1] Starting PRP test of M1000003\n[Worker #2] Starting PRP test of M1000033\n'
    records += ''.join(f'[Worker #{i%2+1}] Iteration: {i+1} / 1000003 [0.10%]\n' for i in range(1000))
    log.write_text(records)
    config.write_text(f'''[observer]
host_id = "measurement-fixture"
database = "{db}"
poll_interval_ms = 200
[[sources]]
id = "workers"
kind = "worker_log"
path = "{log}"
format = "prime95-text-subset-v1"
''')
    def event_count():
        try:
            with sqlite3.connect(db) as conn: return conn.execute('select count(*) from events').fetchone()[0]
        except sqlite3.OperationalError: return 0
    process = subprocess.Popen([binary, 'watch', '--config', config], stdout=subprocess.DEVNULL)
    try:
        wait_for(lambda: event_count() == 1002)
        def cpu():
            fields = pathlib.Path(f'/proc/{process.pid}/stat').read_text().split()
            return (int(fields[13])+int(fields[14]))/os.sysconf('SC_CLK_TCK')
        before, start = cpu(), time.monotonic()
        time.sleep(10)
        elapsed, used = time.monotonic()-start, cpu()-before
        rss = next(int(x.split()[1]) for x in pathlib.Path(f'/proc/{process.pid}/status').read_text().splitlines() if x.startswith('VmRSS:'))/1024
        latencies = []
        for i in range(5):
            started=time.monotonic()
            with log.open('a') as f: f.write(f'[Worker #1] Iteration: {2000+i} / 1000003 [0.20%]\n')
            wait_for(lambda: event_count() == 1003+i)
            latencies.append((time.monotonic()-started)*1000)
        process.kill(); process.wait(timeout=5)
        before_restart=event_count()
        process=subprocess.Popen([binary, 'watch', '--config', config], stdout=subprocess.DEVNULL)
        time.sleep(.5)
        assert event_count() == before_restart, 'restart duplicated records'
        with log.open('a') as f: f.write('[Worker #1] Iteration: 3000 / 1000003 [0.30%]\n')
        wait_for(lambda: event_count() == before_restart+1)
        cpu_name=next(x.split(':',1)[1].strip() for x in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if x.startswith('model name'))
        report={'platform':platform.system()+' '+platform.machine(),'cpu':cpu_name,'rust':subprocess.check_output(['rustc','--version'],text=True).strip(),'fixture_workers':2,'initial_records':1002,'input_bytes':len(records.encode()),'poll_interval_ms':200,'idle_seconds':elapsed,'idle_cpu_percent_of_one_core':used/elapsed*100,'rss_mib':rss,'visibility_ms':latencies,'kill_restart_no_duplicates':True,'scope':'synthetic fixture, no live mprime validation'}
        print(json.dumps(report,indent=2))
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGINT); process.wait(timeout=5)
