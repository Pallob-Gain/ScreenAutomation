"""Tkinter UI: node list with an Add button; each node with a region gets a
frameless, translucent overlay window with a border and the node name as title."""
import tkinter as tk
from tkinter import colorchooser, filedialog, simpledialog, ttk

from .backends import Backends
from .graph import Graph
from .nodes import NODE_TYPES, Node

GRIP = 12


class RegionOverlay(tk.Toplevel):
    """Title-bar-less window marking a capture region. Drag to move, corner to resize."""

    def __init__(self, master, node, on_change):
        super().__init__(master)
        self.node, self.on_change = node, on_change
        self.overrideredirect(True)
        self.attributes("-topmost", True)
        r = node.region
        self.geometry(f"{r.width}x{r.height}+{r.x}+{r.y}")
        self.canvas = tk.Canvas(self, highlightthickness=0)
        self.canvas.pack(fill="both", expand=True)
        self.canvas.bind("<ButtonPress-1>", self._press)
        self.canvas.bind("<B1-Motion>", self._drag)
        self.canvas.bind("<ButtonRelease-1>", lambda e: self._sync())
        self.bind("<Configure>", lambda e: self.redraw())
        self.apply_style()

    def apply_style(self):
        self.attributes("-alpha", self.node.opacity)
        self.canvas.configure(bg=self.node.color)
        self.redraw()

    def redraw(self):
        c = self.canvas
        c.delete("deco")
        w, h = self.winfo_width(), self.winfo_height()
        c.create_rectangle(1, 1, w - 2, h - 2, outline="red", width=3, tags="deco")
        c.create_text(6, 6, text=self.node.name, anchor="nw", fill="black",
                      font=("TkDefaultFont", 10, "bold"), tags="deco")
        c.create_rectangle(w - GRIP, h - GRIP, w, h, fill="red", outline="", tags="deco")

    def _press(self, e):
        self._start = (e.x_root, e.y_root, self.winfo_x(), self.winfo_y(),
                       self.winfo_width(), self.winfo_height())
        self._resize = e.x > self.winfo_width() - GRIP and e.y > self.winfo_height() - GRIP

    def _drag(self, e):
        sx, sy, x, y, w, h = self._start
        dx, dy = e.x_root - sx, e.y_root - sy
        if self._resize:
            self.geometry(f"{max(40, w + dx)}x{max(30, h + dy)}+{x}+{y}")
        else:
            self.geometry(f"{w}x{h}+{x + dx}+{y + dy}")

    def _sync(self):
        r = self.node.region
        r.x, r.y, r.width, r.height = (self.winfo_x(), self.winfo_y(),
                                       self.winfo_width(), self.winfo_height())
        self.on_change()


class App(tk.Tk):
    def __init__(self):
        super().__init__()
        self.title("ScreenAutomation")
        self.graph, self.backends, self.overlays, self.running = Graph(), Backends(), {}, False
        bar = ttk.Frame(self)
        bar.pack(fill="x")
        self.type_var = tk.StringVar(value="ocr")
        ttk.Combobox(bar, textvariable=self.type_var, values=list(NODE_TYPES),
                     state="readonly", width=10).pack(side="left")
        ttk.Button(bar, text="＋ Add", command=self.add_node).pack(side="left")
        ttk.Button(bar, text="Connect", command=self.connect).pack(side="left")
        ttk.Button(bar, text="Delete", command=self.delete).pack(side="left")
        ttk.Button(bar, text="Edit", command=self.edit).pack(side="left")
        ttk.Button(bar, text="Color", command=self.pick_color).pack(side="left")
        ttk.Button(bar, text="Save", command=self.save).pack(side="left")
        ttk.Button(bar, text="Load", command=self.load).pack(side="left")
        self.run_btn = ttk.Button(bar, text="Run", command=self.toggle_run)
        self.run_btn.pack(side="left")
        self.opacity = tk.Scale(bar, from_=0.05, to=1.0, resolution=0.05, orient="horizontal",
                                label="Opacity", command=self.set_opacity)
        self.opacity.pack(side="left")
        self.tree = tk.Listbox(self, width=60, height=15, exportselection=False)
        self.tree.pack(fill="both", expand=True)
        self.tree.bind("<<ListboxSelect>>", lambda e: self.sync_opacity())
        self.refresh_ms = 1000

    def selected(self):
        sel = self.tree.curselection()
        return self.graph.nodes.get(self.tree.get(sel[0]).split("  ")[0]) if sel else None

    def refresh_list(self):
        self.tree.delete(0, "end")
        for n in self.graph.nodes.values():
            outs = ",".join(d for s, d in self.graph.edges if s == n.name)
            self.tree.insert("end", f"{n.name}  [{n.type_name}]" + (f" -> {outs}" if outs else ""))

    def add_node(self):
        t = self.type_var.get()
        name = simpledialog.askstring("Node name", "Name:", initialvalue=f"{t}{len(self.graph.nodes) + 1}")
        if not name:
            return
        try:
            node = self.graph.add_node(NODE_TYPES[t](name))
        except ValueError as e:
            return self.bell() or print(e)
        self.attach(node)
        self.refresh_list()

    def attach(self, node):
        if node.has_region:
            self.overlays[node.name] = RegionOverlay(self, node, lambda: None)

    def connect(self):
        src = self.selected()
        dst = simpledialog.askstring("Connect", "Target node name:")
        if src and dst:
            try:
                self.graph.connect(src.name, dst)
            except KeyError:
                return
            self.refresh_list()

    def delete(self):
        n = self.selected()
        if n:
            ov = self.overlays.pop(n.name, None)
            if ov:
                ov.destroy()
            self.graph.remove_node(n.name)
            self.refresh_list()

    def edit(self):
        n = self.selected()
        if not n:
            return
        key = {"model": "format", "function": "code", "keyboard": "text", "click": "button"}.get(n.type_name)
        if key:
            val = simpledialog.askstring("Configure", key + ":", initialvalue=n.config.get(key, ""))
            if val is not None:
                n.config[key] = val

    def pick_color(self):
        n = self.selected()
        if n and n.has_region:
            c = colorchooser.askcolor(n.color)[1]
            if c:
                n.color = c
                self.overlays[n.name].apply_style()

    def sync_opacity(self):
        n = self.selected()
        if n:
            self.opacity.set(n.opacity)

    def set_opacity(self, v):
        n = self.selected()
        if n and n.has_region and n.name in self.overlays:
            n.opacity = float(v)
            self.overlays[n.name].apply_style()

    def toggle_run(self):
        self.running = not self.running
        self.run_btn.config(text="Stop" if self.running else "Run")
        if self.running:
            self.tick()

    def tick(self):
        if not self.running:
            return
        try:
            self.graph.run_once(self.backends)
        except Exception as e:  # keep the loop alive
            print("run error:", e)
        self.after(self.refresh_ms, self.tick)

    def save(self):
        p = filedialog.asksaveasfilename(defaultextension=".json")
        if p:
            open(p, "w").write(self.graph.to_json())

    def load(self):
        p = filedialog.askopenfilename()
        if p:
            for ov in self.overlays.values():
                ov.destroy()
            self.overlays = {}
            self.graph = Graph.from_json(open(p).read())
            for n in self.graph.nodes.values():
                self.attach(n)
            self.refresh_list()


def main():
    App().mainloop()
