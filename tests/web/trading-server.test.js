const { test } = require("node:test");
const assert = require("node:assert/strict");
const { normalizeChart, server } = require("./trading-server");
const { BreakoutStudy, orderError, command } = require("./breakout-study");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

function response(times, closes) {
  return {
    chart: { result: [{
      meta: {
        symbol: "NVDA", currency: "USD", regularMarketPrice: 125,
        regularMarketTime: 300, previousClose: 120, chartPreviousClose: 100
      },
      timestamp: times,
      indicators: { quote: [{ close: closes }] }
    }] }
  };
}

test("returns provider history, filters missing bars and appends the latest quote", () => {
  const result = normalizeChart(response([100, 150, 200], [121, null, 123]), "NVDA");
  assert.deepEqual(result.history, [
    { time: 100, price: 121 }, { time: 200, price: 123 }, { time: 300, price: 125 }
  ]);
  assert.equal(result.previousClose, 120);
});

test("replaces a matching latest bar instead of creating a duplicate timestamp", () => {
  const result = normalizeChart(response([100, 300], [121, 124]), "NVDA");
  assert.deepEqual(result.history, [{ time: 100, price: 121 }, { time: 300, price: 125 }]);
});

test("rejects mismatched symbols and unusable histories instead of inventing prices", () => {
  assert.throws(() => normalizeChart(response([100], [121]), "AAPL"), /invalid quote fields/);
  assert.throws(() => normalizeChart(response([100], [null]), "NVDA"), /No valid historical prices/);
});

function market(price = 105) {
  return { symbol: "NVDA", range: "1d", price, time: 3010,
    bars: Array.from({ length: 21 }, (_, index) => ({
      time: 1800 + index * 60, high: index === 20 ? 1000 : 110, low: 100
    })) };
}

test("completed minute highs/lows exclude the current candle and phases retain frozen hysteresis levels", () => {
  const study = new BreakoutStudy();
  const update = (price, overrides = {}) => study.update({ ...market(price), ...overrides }, 3010000);
  const idle = update(105);
  assert.equal(idle.phase, "Idle");
  assert.deepEqual(idle.levels, { support: 100, resistance: 110, buyStart: 110.11, sellStart: 99.9 });
  assert.equal(update(110.11).phase, "Idle");
  assert.equal(update(110.12).phase, "Buy");
  const changed = market().bars.map((bar) => ({ ...bar, low: 90, high: 200 }));
  assert.equal(update(110.05, { bars: changed }).levels.resistance, 110);
  assert.equal(update(110).phase, "Buy");
  assert.equal(update(109.99).phase, "Idle");
  assert.equal(update(99.9).phase, "Idle");
  assert.equal(update(99.89).phase, "Sell");
  assert.equal(update(100).phase, "Sell");
  assert.equal(update(100.01).phase, "Idle");
});

test("stale, missing, wrong-timeframe and malformed history disables signals and clears active phases", () => {
  for (const invalid of [
    { time: 2919 }, { time: 3011 }, { range: "5d" }, { symbol: "AAPL" },
    { bars: market().bars.slice(2) }, { bars: [null] }, { bars: {} },
    { bars: market().bars.filter((bar) => bar.time !== 2400) }, { error: "Quote unavailable" }
  ]) {
    const study = new BreakoutStudy();
    assert.equal(study.update(market(111), 3010000).phase, "Buy");
    const result = study.update({ ...market(111), ...invalid }, 3010000);
    assert.equal(result.ready, false);
    assert.equal(result.phase, "Idle");
    assert.equal(result.levels, null);
    assert.ok(result.reason);
  }
});

test("paper signals repeat with changing holdings, stop at ten/zero, and cannot bypass busy or cooldown guards", () => {
  const state = { enabled: true, latched: true,
    snapshot: new BreakoutStudy().update(market(111), 3010000),
    shares: 0, hasOpenOrders: false, now: 3000, lastSubmittedAt: 2000, cooldownUntil: 3000,
    busy: false, price: 11100, buyingPower: 10000000 };
  for (let shares = 0; shares < 10; shares++) {
    assert.equal(command({ ...state, shares }), `BUY NVDA 1 POSITION ${shares}`);
  }
  assert.equal(command({ ...state, shares: 10 }), "WAIT");
  const selling = { ...state, snapshot: { ...state.snapshot, phase: "Sell" } };
  for (let shares = 10; shares > 0; shares--) {
    assert.equal(command({ ...selling, shares }), `SELL NVDA 1 POSITION ${shares}`);
  }
  assert.equal(command({ ...selling, shares: 0 }), "WAIT");
  for (const blocked of [
    { busy: true }, { enabled: false }, { cooldownUntil: 3001 }, { lastSubmittedAt: 2001 },
    { hasOpenOrders: true }, { shares: -1 }, { shares: 11 }, { buyingPower: 11099 }
  ]) assert.equal(command({ ...state, ...blocked }), "WAIT");
  const order = { side: "Buy", symbol: "NVDA", quantity: 1, type: "Market" };
  assert.equal(orderError(order, state), "");
  for (const change of [{ symbol: "AAPL" }, { quantity: 2 }, { type: "Limit" }, { side: "Sell" }]) {
    assert.ok(orderError({ ...order, ...change }, state));
  }
  assert.ok(orderError(order, { ...state, enabled: false }), "disarming must block an already reviewed order");
  assert.ok(orderError(order, { ...state, snapshot: { ...state.snapshot, phase: "Idle" } }));
});

test("provider candle extremes remain independent of the appended latest quote", () => {
  const data = response([60, 120, 180], [121, 122, 123]);
  Object.assign(data.chart.result[0].indicators.quote[0], { high: [125, null, 124], low: [120, 119, 122] });
  assert.deepEqual(normalizeChart(data, "NVDA").bars, [
    { time: 60, high: 125, low: 120 }, { time: 180, high: 124, low: 122 }
  ]);
});

test("local server serves the indicator module without fetching a market quote", async () => {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const response = await fetch(`http://127.0.0.1:${server.address().port}/breakout-study.js`);
    assert.equal(response.status, 200);
    assert.match(response.headers.get("content-type"), /text\/javascript/);
    assert.match(await response.text(), /class BreakoutStudy/);
  } finally {
    await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  }
});

test("actual paper-page handlers recheck phases, enforce ten/zero holdings, and wait a full second after confirmation", async () => {
  class Element {
    constructor() {
      this.value = ""; this.checked = false; this.open = false;
      this.dataset = {}; this.handlers = {}; this.children = {};
      this.classList = { toggle() {} };
    }
    addEventListener(name, handler) { (this.handlers[name] ||= []).push(handler); }
    fire(name, event = {}) { for (const handler of this.handlers[name] || []) handler(event); }
    setAttribute() {}
    querySelector(key) { return this.children[key] ||= new Element(); }
    showModal() { this.open = true; }
    close() { this.open = false; this.fire("close"); }
  }
  const elements = {};
  const element = (id) => elements[id] ||= new Element();
  element("instrument").value = "NVDA";
  element("orderType").value = "Market";
  element("quantity").value = "1";
  let clock = 3010000, price = 111;
  const timers = [];
  class ClockDate extends Date { static now() { return clock; } }
  const source = fs.readFileSync(path.join(__dirname, "trading.html"), "utf8").match(/<script>([\s\S]*?)<\/script>/)[1];
  vm.runInNewContext(source, {
    document: { getElementById: element, querySelectorAll: () => [] },
    location: { protocol: "http:", search: "?breakout=1" },
    URLSearchParams, Date: ClockDate, PaperBreakout: { BreakoutStudy, orderError, command },
    AbortSignal: { timeout() {} }, setInterval: (callback) => timers.push(callback),
    fetch: async (url) => ({
      ok: true, json: async () => ({
        ...market(price), symbol: new URL(url, "http://localhost").searchParams.get("symbol"),
        history: market(price).bars.map((bar) => ({ time: bar.time, price: 105 }))
      })
    })
  });
  const settled = () => new Promise(setImmediate);
  await settled();
  element("breakoutEnabled").checked = true;
  element("breakoutEnabled").fire("change");
  assert.equal(element("breakoutCommand").textContent, "BUY NVDA 1 POSITION 0");
  assert.equal(element("confirmationDialog").open, false, "enabling the indicator must never place an order");
  element("reviewOrder").fire("click");
  assert.equal(element("breakoutCommand").textContent, "WAIT");
  price = 109.99;
  timers[0]();
  await settled();
  element("confirmOrder").fire("click");
  assert.match(element("reviewError").textContent, /phase/);
  assert.equal(Number(element("sharesOwned").textContent), 0, "price reversal must reject the reviewed buy");
  element("reviewDialog").close();
  price = 111;
  timers[0]();
  await settled();
  const trade = () => {
    element("reviewOrder").fire("click");
    element("confirmOrder").fire("click");
    assert.equal(element("confirmationDialog").open, true);
    element("confirmationDialog").close();
    assert.equal(element("breakoutCommand").textContent, "WAIT");
    clock += 999;
    timers[1]();
    assert.equal(element("breakoutCommand").textContent, "WAIT");
    clock++;
    timers[1]();
  };
  for (let held = 1; held <= 10; held++) {
    trade();
    assert.equal(Number(element("sharesOwned").textContent), held);
  }
  assert.equal(element("breakoutCommand").textContent, "WAIT");
  element("reviewOrder").fire("click");
  assert.match(element("orderMessage").textContent, /10 shares/);
  price = 99.89;
  timers[0]();
  await settled();
  element("sell").fire("click");
  for (let held = 9; held >= 0; held--) {
    trade();
    assert.equal(Number(element("sharesOwned").textContent), held);
  }
  assert.equal(element("breakoutCommand").textContent, "WAIT");
  element("reviewOrder").fire("click");
  assert.match(element("orderMessage").textContent, /Not enough available shares/);
  element("breakoutEnabled").checked = false;
  element("breakoutEnabled").fire("change");
  element("buy").fire("click");
  element("reviewOrder").fire("click");
  assert.match(element("orderMessage").textContent, /disarmed/);
  element("breakoutEnabled").checked = true;
  element("breakoutEnabled").fire("change");
  clock += 91000;
  timers[1]();
  assert.equal(element("breakoutCommand").textContent, "WAIT");
  assert.equal(element("breakoutReason").textContent, "A provider quote no older than 90 seconds is required.");
});
