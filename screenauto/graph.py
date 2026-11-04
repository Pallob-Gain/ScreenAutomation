"""Node graph and execution engine."""
import json

from .nodes import Node


class Graph:
    def __init__(self):
        self.nodes = {}   # name -> Node
        self.edges = []   # (source, target)

    def add_node(self, node):
        if node.name in self.nodes:
            raise ValueError(f"duplicate node name: {node.name}")
        self.nodes[node.name] = node
        return node

    def remove_node(self, name):
        del self.nodes[name]
        self.edges = [e for e in self.edges if name not in e]

    def connect(self, src, dst):
        if src not in self.nodes or dst not in self.nodes:
            raise KeyError("unknown node")
        if (src, dst) not in self.edges:
            self.edges.append((src, dst))

    def _order(self):
        indeg = {n: 0 for n in self.nodes}
        for _, d in self.edges:
            indeg[d] += 1
        queue = [n for n, c in indeg.items() if c == 0]
        order = []
        while queue:
            n = queue.pop(0)
            order.append(n)
            for s, d in self.edges:
                if s == n:
                    indeg[d] -= 1
                    if indeg[d] == 0:
                        queue.append(d)
        if len(order) != len(self.nodes):
            raise ValueError("graph contains a cycle")
        return order

    def run_once(self, backends):
        """One refresh tick. Nodes with no output (None) stop propagation."""
        outputs = {}
        for name in self._order():
            ins = [outputs[s] for s, d in self.edges if d == name and outputs.get(s) is not None]
            has_parents = any(d == name for _, d in self.edges)
            if has_parents and not ins:
                outputs[name] = None
                continue
            outputs[name] = self.nodes[name].process(ins, backends)
        return outputs

    def to_json(self):
        return json.dumps({"nodes": [n.to_dict() for n in self.nodes.values()],
                           "edges": self.edges}, indent=2)

    @classmethod
    def from_json(cls, text):
        data = json.loads(text)
        g = cls()
        for d in data["nodes"]:
            g.add_node(Node.from_dict(d))
        for s, d in data["edges"]:
            g.connect(s, d)
        return g
