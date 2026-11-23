const { test } = require("node:test");
const assert = require("node:assert/strict");
const { normalizeChart } = require("./trading-server");

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
