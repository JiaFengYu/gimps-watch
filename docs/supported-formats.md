# Supported input adapters

This is a compatibility **subset**, not a release-level Prime95 support claim. Input examples are constructed and must be checked against the target installation. Reference documentation for collecting actual results: <https://www.mersenne.org/download/> and <https://www.mersenne.org/various/math.php>. These sites were unavailable from the implementation environment; their current format/usage guidance was not fetched.

## `prime95-text-subset-v1` / `worker_log`

One record per newline. LF and CRLF are accepted. Optional leading `[Worker #N]` or `[Worker #N ...]` identifies `worker-N`; bracket contents after N are not parsed as timestamps. Alternatively configure one worker per source. A conflicting prefix produces a diagnostic.

Accepted bodies:

```text
Starting PRP test of M1000003 using example parameters.
Starting primality test of M1000003
Starting Lucas-Lehmer test of M1000003
Iteration: 1000 / 1000003 [0.10%], ms/iter: 2.50
M1000003 is not prime. Res64: 0123456789ABCDEF
M1000003 is a probable prime.
M1000003 is prime!
ERROR: an explicit error
FATAL ERROR: an explicit fatal error
```

Assignment/completion supply an exponent; no assignment UID is inferred. Progress supplies iteration, denominator, and percentage; numeric bounds are checked. A progress record can inherit the last known assignment from the same configured worker. It is unknown if that worker has not reported a recognized start. An unsupported start line cannot establish a new assignment. Keep each logical worker ID unique across sources.

No extrapolation from FFT timings, work queues, quiet logs, or process lists. Event time is always null because the adapter does not infer timezones from log prefixes. Observation time is a separate field.

## `prime95-json-subset-v1` / `results`

One JSON object per newline. Accepted composite-result subset:

```json
{"status":"C","exponent":1000003,"worktype":"PRP-3","res64":"0123456789ABCDEF","aid":"0123456789abcdef0123456789abcdef"}
```

Required: `status = "C"`, integer exponent >=2, nonempty worktype, 16-digit hexadecimal res64. Optional `aid`: a 32-digit hexadecimal assignment ID. Other JSON fields are retained in raw provenance but not interpreted. Other statuses/result forms produce diagnostics, including results without `res64`. `C` means a composite result in this subset, not a generic acceptance of every completed-work format.

An explicit completion deduplication key consists of assignment UID, worktype, and normalized residue. This deduplicates exact known results; conflicting results for one assignment remain separate records. Without a valid UID the result is retained with `exponent_only_ambiguous` confidence. No authoritative unique-work total is produced.

## Reader contract

Default maximum record: 65,536 bytes including terminator. Per poll the reader allocates at most twice the configured record maximum plus one byte and processes each complete bounded record. An oversized record blocks that source with a visible health message; raise the limit (maximum 1 MiB) or correct the source. Previously committed complete records remain available.

The checkpoint includes device/inode, generation, offset, and SHA-256 of up to the last 128 committed bytes. Complete events and checkpoint updates are atomic. Diagnostic records are retained just like supported input, except they do not change worker state.
