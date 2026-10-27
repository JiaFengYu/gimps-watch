import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const browser=await chromium.launch({executablePath:process.env.CHROME_BIN || undefined,headless:true,args:['--no-sandbox','--disable-dev-shm-usage']});
const page=await browser.newPage({viewport:{width:1280,height:900}});const errors=[];page.on('pageerror',e=>errors.push(e.message));
try{
  await page.goto(process.env.DEMO_URL || 'http://127.0.0.1:8787');
  await page.waitForFunction(()=>document.querySelectorAll('#workers .card').length===2);
  assert.ok((await page.locator('#workers').textContent()).includes('completion observed'));
  assert.ok((await page.locator('#workers').textContent()).includes('Example hardware error'));
  assert.equal(await page.locator('#sources .card').count(),2);
  assert.equal(await page.locator('#events details').count(),9);
  await page.locator('#events summary').first().click();assert.ok((await page.locator('#events pre').first().textContent()).includes('byte_offset'));
  await page.screenshot({path:'docs/dashboard.png',fullPage:true});
  await page.setViewportSize({width:390,height:844});assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'mobile horizontal overflow');
  await page.screenshot({path:'docs/dashboard-mobile.png',fullPage:true});
  await page.locator('#refresh').click();assert.deepEqual(errors,[]);
  console.log(JSON.stringify({passed:true,checks:['2 observed workers','completion and error states','source health','9 raw records','provenance disclosure','390px viewport','refresh','no JS errors']}));
}finally{await browser.close();}
