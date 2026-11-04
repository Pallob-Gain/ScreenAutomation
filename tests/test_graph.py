import unittest

from screenauto.graph import Graph
from screenauto.nodes import ClickNode, FunctionNode, KeyboardNode, ModelNode, OCRNode


class FakeImage:
    def __init__(self, data):
        self.data = data

    def tobytes(self):
        return self.data.encode()


class FakeBackends:
    def __init__(self):
        self.screen, self.typed, self.clicks = "Price: 42", [], []

    def capture(self, region):
        return FakeImage(self.screen)

    def ocr(self, image):
        return image.data

    def type_text(self, text):
        self.typed.append(text)

    def click(self, x, y, button="left"):
        self.clicks.append((x, y, button))


def build():
    g = Graph()
    g.add_node(OCRNode("ocr"))
    g.add_node(ModelNode("num", format="number"))
    g.add_node(FunctionNode("fn", code="result = 'go' if inputs[0] > 10 else None"))
    g.add_node(KeyboardNode("kb"))
    g.add_node(ClickNode("click"))
    for a, b in [("ocr", "num"), ("num", "fn"), ("fn", "kb"), ("fn", "click")]:
        g.connect(a, b)
    return g


class GraphTest(unittest.TestCase):
    def test_pipeline_and_change_detection(self):
        g, b = build(), FakeBackends()
        g.run_once(b)
        self.assertEqual(b.typed, ["go"])
        self.assertEqual(len(b.clicks), 2)  # focus click + click node
        g.run_once(b)  # unchanged screen -> nothing happens
        self.assertEqual(b.typed, ["go"])
        b.screen = "Price: 5"  # changed, function returns None
        g.run_once(b)
        self.assertEqual(b.typed, ["go"])
        b.screen = "Price: 99"
        g.run_once(b)
        self.assertEqual(b.typed, ["go", "go"])

    def test_roundtrip_and_cycle(self):
        g = build()
        g.nodes["kb"].region.x = 5
        g2 = Graph.from_json(g.to_json())
        self.assertEqual(g2.nodes["kb"].region.x, 5)
        self.assertEqual(g2.edges, g.edges)
        g.connect("kb", "ocr")
        with self.assertRaises(ValueError):
            g.run_once(FakeBackends())


if __name__ == "__main__":
    unittest.main()
