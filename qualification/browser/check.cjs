const {spawn} = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const [fixtureBin, assets, output, playwrightModule, browserBin] = process.argv.slice(2);
const {chromium} = require(playwrightModule);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function ownedFixture() {
  const child = spawn(fixtureBin, [assets], {stdio: ['ignore', 'pipe', 'pipe']});
  let ready = '';
  child.stdout.on('data', bytes => ready += bytes);
  child.stderr.on('data', bytes => fs.appendFileSync(path.join(output, 'fixture.log'), bytes));
  const deadline = Date.now() + 10000;
  while (!ready.includes('\n')) {
    if (child.exitCode !== null || Date.now() >= deadline) { child.kill(); throw Error('fixture startup failed'); }
    await sleep(20);
  }
  return {child, settings: JSON.parse(ready.split('\n')[0])};
}

(async () => {
  const browser = await chromium.launch({executablePath: browserBin, headless: true});
  const results = [];
  try {
    for (const scenario of ['changing', 'binding', 'retention', 'expiry', 'static', 'websocket']) {
      const {child, settings} = await ownedFixture();
      const context = await browser.newContext();
      const page = await context.newPage();
      page.on('pageerror', error => fs.appendFileSync(path.join(output, 'page-errors.log'), String(error) + '\n'));
      try {
        await page.goto(settings.origin + '/assets/index.html');
        await page.waitForFunction(() => window.ready);
        assert.equal(await page.evaluate(() => isSecureContext), true);
        assert.equal(await page.evaluate(() => window.valid_frame_size(1920, 1080, 7680, 8294400)), true);
        assert.equal(await page.evaluate(() => window.valid_frame_size(1921, 1080, 7684, 8298720)), false);
        assert.equal(await page.evaluate(() => window.valid_frame_size(1, 1, 3, 3)), false);
        if (scenario === 'binding') {
          const result = await page.evaluate(settings => window.binding_proof_failures(settings), settings);
          assert.deepEqual(result, {invalid: 'invalid_binding_challenge', expired: 401, changed: 401, wrongKey: 401, wrongSubject: 401, valid: 200, replay: 401, closed: 'cancelled'});
          results.push({scenario, ...result});
        } else if (scenario === 'retention') {
          const result = await page.evaluate(async settings => {
            const transport = await window.raw_transport(settings);
            const first = await transport.next_packet();
            const second = await transport.next_packet();
            let busy, repeat, stale;
            try { await transport.next_packet(); } catch (error) { busy = String(error); }
            const bytes = first.take_payload();
            try { first.take_payload(); } catch (error) { repeat = String(error); }
            await transport.close();
            try { second.take_payload(); } catch (error) { stale = String(error); }
            first.free(); second.free(); transport.free();
            return {busy, repeat, stale, bytes: bytes.length};
          }, settings);
          assert.equal(result.busy, 'frame_busy');
          assert.equal(result.repeat, 'payload_already_taken');
          assert.equal(result.stale, 'cancelled');
          assert.ok(result.bytes > 0 && result.bytes <= 8388608);
          results.push({scenario, ...result});
        } else if (scenario === 'websocket') {
          for (const endpoint of ['overflow', 'text-overflow', 'burst']) {
            const result = await page.evaluate(url => window.receiver_bounds_probe(url), settings.origin.replace('http:', 'ws:') + '/' + endpoint);
            assert.equal(result, 'closed');
            results.push({scenario, endpoint, result});
          }
        } else {
          await page.evaluate(settings => window.start(settings), {...settings, lifetime: scenario === 'expiry' ? 3000 : 20000});
          await page.waitForFunction(() => window.stats().drawn > 0);
          const pixel = () => page.evaluate(() => Array.from(document.querySelector('canvas').getContext('2d').getImageData(100, 100, 1, 1).data));
          let observed = await pixel();
          assert.ok(observed[0] > 180 && observed[2] < 70);
          if (scenario === 'changing') {
            await page.screenshot({path: path.join(output, 'red.png')});
            await page.request.post(settings.origin + '/color', {headers: {authorization: `Bearer ${settings.cap}`}, data: {blue: true}});
            await page.waitForFunction(() => {
              const pixel = document.querySelector('canvas').getContext('2d').getImageData(100, 100, 1, 1).data;
              return pixel[2] > 180 && pixel[0] < 70;
            });
            await page.screenshot({path: path.join(output, 'blue.png')});
            observed = await pixel();
          } else if (scenario === 'static') {
            await sleep(8000);
            assert.equal(await page.evaluate(() => window.view.is_current()), true);
            assert.ok((await page.evaluate(() => window.stats())).drawn >= 4);
          } else {
            await page.waitForFunction(() => window.stats().error !== null, {timeout: 6000});
            assert.equal(await page.evaluate(() => window.view.is_current()), false);
          }
          const beforeClose = await page.evaluate(() => window.stats());
          assert.equal(beforeClose.decoded, beforeClose.closed);
          await page.evaluate(() => window.stop());
          assert.deepEqual(await pixel(), [0, 0, 0, 0]);
          results.push({scenario, observed, beforeClose, cleared: true});
        }
      } finally {
        await context.close();
        child.kill();
        await new Promise(resolve => child.once('exit', resolve));
      }
    }
    fs.writeFileSync(path.join(output, 'browser-results.json'), JSON.stringify({browser: browser.version(), results}, null, 2) + '\n');
    console.log(`${results.length} browser boundary results passed`);
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
