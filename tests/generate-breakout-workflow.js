const fs = require("node:fs");
const path = require("node:path");

function buildWorkflow() {
  const base = JSON.parse(fs.readFileSync(path.join(__dirname, "trading-buy-sell.calibrated.json"), "utf8"));
  const nodes = [], edges = [];
  const copy = (source, name = source, changes = {}) => {
    const node = structuredClone(base.nodes.find((node) => node.name === source));
    if (!node) throw new Error(`Missing template node: ${source}`);
    Object.assign(node, { name }, changes);
    nodes.push(node);
    return name;
  };
  const add = (node) => { nodes.push({ color: [130, 205, 170], opacity: .15, region: null, ...node }); return node.name; };
  const chain = (names) => names.slice(1).forEach((name, index) => edges.push([names[index], name]));
  copy("setup", "setup", { title: "PAPER BREAKOUT: READ BEFORE ARMING",
    text: "PAPER ONLY. Open http://127.0.0.1:8765/?breakout=1 with NVDA / Market / 1D selected. Enable guarded paper signals in the indicator and set workflow_armed to true only after checking every region. Uses 20 completed one-minute highs/lows, 0.1% breakout buffer, frozen phase levels. Repeats one-share orders; max 10 held; no shorts; one-second cooldown after confirmation closes. Stale (>90s), missing or incomplete history blocks orders. Price feeds may be delayed; closed markets do not signal. Never connect to a real account. Stop and inspect history after any failure. Reload resets the paper account and the indicator; do not reload an account you want to preserve. Main ticket/review regions reuse the existing calibrated display (125% Windows, 67% Edge zoom, 1920x1140 browser including chrome). Indicator OCR boxes require verification for your display. This JSON is saved disarmed." });
  add({ name: "workflow_armed", title: "ARM WORKFLOW: false = disarmed", group: "Control",
    type: "constant", value: "false", format: "bool" });
  copy("price", "signal", { title: "Paper indicator command only", group: "Control",
    region: { x: 28, y: 730, width: 265, height: 27 } });
  add({ name: "direction", title: "Allow only validated NVDA one-share signals", group: "Control", type: "function",
    code: 'if !inputs[0] || inputs.len() < 2 { "WAIT" } else { let s = inputs[1]; s.trim(); let result = "IGNORED OCR: " + s; if s == "WAIT" { result = "WAIT"; } for n in 0..10 { if s == "BUY NVDA 1 POSITION " + n { result = "BUY"; } } for n in 1..11 { if s == "SELL NVDA 1 POSITION " + n { result = "SELL"; } } result }' });
  edges.push(["workflow_armed", "direction"], ["signal", "direction"]);
  for (const side of ["Buy", "Sell"]) {
    const prefix = side === "Buy" ? "" : "sell_";
    const branch = add({ name: `${prefix}active`, title: `${side} phase`, group: `${side} ticket`,
      type: "compare", op: "==", value: side.toUpperCase(), on_change: false });
    edges.push(["direction", branch]);
    const marker = copy("paper_only", `${prefix}indicator_armed`, { title: "Guarded paper indicator is armed",
      group: `${side} ticket`, expected: "PAPER BREAKOUT ARMED", matching: "exact", single_line: true,
      region: { x: 28, y: 709, width: 265, height: 22 } });
    const paper = copy("paper_only", `${prefix}paper_only`, { group: `${side} ticket` });
    const held = copy("check_empty_position", `${prefix}holding_limit`, {
      title: side === "Buy" ? "Own 0-9 shares before buying" : "Own 1-10 shares before selling",
      group: `${side} ticket`, matching: "regex", expected: side === "Buy" ? "^[0-9]$" : "^([1-9]|10)$" });
    const names = [branch, marker, paper, held];
    for (const source of side === "Buy"
      ? ["buy_tab", "ticket_ready", "quantity", "quantity_ready", "review", "review_ready", "check_symbol", "check_side", "check_quantity", "check_type", "check_total"]
      : ["sell_tab", "sell_ticket_ready", "sell_quantity", "sell_quantity_ready", "sell_review", "sell_review_ready", "sell_check_symbol", "sell_check_side", "sell_check_quantity", "sell_check_type", "sell_check_total"]) names.push(copy(source));
    for (const suffix of ["submit", "filled", "check_fill", "done"]) names.push(copy(`${prefix}${suffix}`));
    chain(names);
  }
  const columns = { Control: 0, "Buy ticket": 1, "Buy review": 2, "Buy result": 3,
    "Sell ticket": 4, "Sell review": 5, "Sell result": 6 };
  const rows = {};
  for (const node of nodes) {
    if (node.type === "note") { node.position = [-400, 0]; continue; }
    const group = node.group || "Control";
    node.position = [columns[group] * 360, (rows[group] = (rows[group] || 0) + 1) * 100];
    if (node.type === "wait_for_text") node.poll_ms = 100;
  }
  return { nodes, edges, groups: Object.keys(columns), settings: { run_mode: "timed", refresh_ms: 1000 } };
}
if (require.main === module) {
  fs.writeFileSync(path.join(__dirname, "trading-breakout.json"), JSON.stringify(buildWorkflow(), null, 2) + "\n");
}
module.exports = { buildWorkflow };
