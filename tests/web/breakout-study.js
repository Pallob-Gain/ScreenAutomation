(function (root) {
  "use strict";
  class BreakoutStudy {
    constructor() { this.reset(); }
    reset() {
      this.phase = "Idle";
      this.levels = null;
    }
    update(data, now = Date.now()) {
      const wait = (reason) => {
        this.reset();
        return { ready: false, phase: this.phase, levels: null, reason };
      };
      if (data.symbol !== "NVDA" || data.range !== "1d") return wait("Select NVDA and the 1D / 1-minute chart.");
      const age = now / 1000 - data.time;
      if (data.error || !Number.isFinite(data.price) || data.price <= 0 || !Number.isFinite(data.time) ||
          !Number.isFinite(age) || age < 0 || age > 90) {
        return wait("A provider quote no older than 90 seconds is required.");
      }
      const cutoff = Math.floor(data.time / 60) * 60;
      if (!Array.isArray(data.bars) || data.bars.some((bar) =>
        !bar || !Number.isSafeInteger(bar.time) || bar.time % 60 !== 0 ||
        !Number.isFinite(bar.high) || !Number.isFinite(bar.low) ||
        bar.low <= 0 || bar.high < bar.low)) return wait("Invalid one-minute history.");
      const bars = data.bars.filter((bar) => bar.time + 60 <= cutoff).slice(-20);
      if (bars.length !== 20 || bars.some((bar, index) =>
        (index > 0 && bar.time - bars[index - 1].time !== 60)) ||
        bars[19].time + 60 !== cutoff) {
        return wait("Waiting for 20 consecutive completed one-minute bars.");
      }
      if ((this.phase === "Buy" && data.price < this.levels.resistance) ||
          (this.phase === "Sell" && data.price > this.levels.support)) this.reset();
      if (this.phase === "Idle") {
        const support = Math.min(...bars.map((bar) => bar.low));
        const resistance = Math.max(...bars.map((bar) => bar.high));
        const rounded = (price) => Math.round(price * 1e6) / 1e6;
        this.levels = { support, resistance,
          buyStart: rounded(resistance * 1.001), sellStart: rounded(support * .999) };
        if (data.price > this.levels.buyStart) this.phase = "Buy";
        else if (data.price < this.levels.sellStart) this.phase = "Sell";
      }
      return { ready: true, phase: this.phase, levels: { ...this.levels },
        reason: this.phase === "Idle" ? "Waiting for a breakout." : `${this.phase} phase; levels frozen.` };
    }
  }

  function orderError(order, state) {
    if (!state.latched && !state.enabled) return "";
    if (!state.enabled) return "Paper breakout is disarmed. Reload to return to unrestricted manual paper trading.";
    if (!state.snapshot.ready) return state.snapshot.reason;
    if (order.symbol !== "NVDA" || order.type !== "Market" || order.quantity !== 1) {
      return "Paper breakout permits only one-share NVDA Market orders.";
    }
    if (order.side !== state.snapshot.phase) return "The breakout phase no longer permits this order.";
    if (!Number.isSafeInteger(state.shares) || state.shares < 0 || state.shares > 10 || state.hasOpenOrders) {
      return "Use 0-10 NVDA shares and no open orders.";
    }
    if (order.side === "Buy" && state.shares >= 10) return "Maximum holding of 10 shares reached.";
    if (order.side === "Sell" && state.shares < 1) return "No shares available; short selling is disabled.";
    if (state.now < state.cooldownUntil || state.now - state.lastSubmittedAt < 1000) {
      return "Waiting for the one-second order cooldown.";
    }
    return "";
  }

  function command(state) {
    if (state.busy) return "WAIT";
    const side = state.snapshot.phase;
    const order = { symbol: "NVDA", type: "Market", quantity: 1, side };
    if (!state.enabled || !["Buy", "Sell"].includes(side) || orderError(order, state)) return "WAIT";
    if (side === "Buy" && state.buyingPower < state.price) return "WAIT";
    return `${side.toUpperCase()} NVDA 1 POSITION ${state.shares}`;
  }
  const api = { BreakoutStudy, orderError, command };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.PaperBreakout = api;
})(globalThis);
