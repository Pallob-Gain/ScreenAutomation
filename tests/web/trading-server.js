const http = require("http");
const https = require("https");
const fs = require("fs");
const path = require("path");

const port = 8765;
const page = path.join(__dirname, "trading.html");
const symbols = new Set(["NVDA", "AAPL"]);
const intervals = { "1d": "1m", "5d": "5m", "1mo": "30m" };

function normalizeChart(data, symbol) {
  const result = data.chart?.result?.[0];
  const meta = result?.meta;
  if (meta?.symbol !== symbol || meta.currency !== "USD" ||
      !Number.isFinite(meta.regularMarketPrice) || meta.regularMarketPrice <= 0 ||
      !Number.isFinite(meta.regularMarketTime) || meta.regularMarketTime <= 0) {
    throw new Error("Missing or invalid quote fields");
  }
  const closes = result.indicators?.quote?.[0]?.close;
  if (!Array.isArray(result.timestamp) || !Array.isArray(closes)) {
    throw new Error("Missing intraday history");
  }
  const history = result.timestamp.flatMap((time, index) =>
    Number.isFinite(time) && time > 0 && Number.isFinite(closes[index]) && closes[index] > 0
      ? [{ time, price: closes[index] }] : []
  ).sort((a, b) => a.time - b.time);
  if (!history.length) throw new Error("No valid historical prices");
  if (meta.regularMarketTime >= history[history.length - 1].time) {
    const latest = { time: meta.regularMarketTime, price: meta.regularMarketPrice };
    if (latest.time === history[history.length - 1].time) history[history.length - 1] = latest;
    else history.push(latest);
  }
  const positive = (value) => Number.isFinite(value) && value > 0 ? value : null;
  const quoteBars = result.indicators.quote[0];
  const bars = result.timestamp.flatMap((time, index) => {
    const high = quoteBars.high?.[index], low = quoteBars.low?.[index], close = closes[index];
    return Number.isSafeInteger(time) && time > 0 && time % 60 === 0 &&
      [high, low, close].every((price) => Number.isFinite(price) && price > 0) &&
      low <= close && close <= high ? [{ time, high, low }] : [];
  }).sort((a, b) => a.time - b.time);
  return {
    symbol,
    price: meta.regularMarketPrice,
    time: meta.regularMarketTime,
    previousClose: positive(meta.previousClose) ?? positive(meta.chartPreviousClose),
    high: positive(meta.regularMarketDayHigh),
    low: positive(meta.regularMarketDayLow),
    volume: positive(meta.regularMarketVolume),
    history,
    bars
  };
}

function sendJson(res, status, data) {
  res.writeHead(status, { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store" });
  res.end(JSON.stringify(data));
}

function quote(symbol, range, res) {
  const url = `https://query1.finance.yahoo.com/v8/finance/chart/${symbol}?interval=${intervals[range]}&range=${range}`;
  const request = https.get(url, { headers: { "User-Agent": "Mozilla/5.0" }, timeout: 10000 }, (upstream) => {
    if (upstream.statusCode !== 200) {
      upstream.resume();
      sendJson(res, 502, { error: `Quote provider returned HTTP ${upstream.statusCode}.` });
      return;
    }
    let body = "";
    upstream.setEncoding("utf8");
    upstream.on("data", (chunk) => {
      body += chunk;
      if (body.length > 1000000) upstream.destroy(new Error("Quote response too large"));
    });
    upstream.on("error", (error) => {
      if (!res.headersSent) sendJson(res, 502, { error: `Quote response failed: ${error.message}` });
    });
    upstream.on("end", () => {
      try {
        sendJson(res, 200, { ...normalizeChart(JSON.parse(body), symbol), range });
      } catch (error) {
        sendJson(res, 502, { error: `Invalid quote response: ${error.message}` });
      }
    });
  });
  request.on("timeout", () => request.destroy(new Error("Quote request timed out")));
  request.on("error", (error) => {
    if (!res.headersSent) sendJson(res, 502, { error: `Quote request failed: ${error.message}` });
  });
}

const server = http.createServer((req, res) => {
  if (req.method !== "GET") {
    sendJson(res, 405, { error: "Method not allowed." });
    return;
  }
  const url = new URL(req.url, `http://127.0.0.1:${port}`);
  if (url.pathname === "/" || url.pathname === "/trading.html" || url.pathname === "/breakout-study.js") {
    const script = url.pathname === "/breakout-study.js";
    fs.readFile(script ? path.join(__dirname, "breakout-study.js") : page, (error, content) => {
      if (error) {
        sendJson(res, 500, { error: `Could not read ${script ? "indicator module" : "trading page"}: ${error.message}` });
        return;
      }
      res.writeHead(200, { "Content-Type": script ? "text/javascript; charset=utf-8" : "text/html; charset=utf-8", "Cache-Control": "no-store" });
      res.end(content);
    });
  } else if (url.pathname === "/api/quote" && symbols.has(url.searchParams.get("symbol"))) {
    const range = url.searchParams.get("range") || "1d";
    if (!Object.hasOwn(intervals, range)) {
      sendJson(res, 400, { error: "Unsupported chart range." });
      return;
    }
    quote(url.searchParams.get("symbol"), range, res);
  } else {
    sendJson(res, 404, { error: "Not found." });
  }
});

if (require.main === module) {
  server.listen(port, "127.0.0.1", () => {
    console.log(`Paper trading page: http://127.0.0.1:${port}/`);
  });
}

module.exports = { normalizeChart, server };
