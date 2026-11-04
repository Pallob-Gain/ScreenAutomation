"""Node types: OCR screen reader, model, function, keyboard and click."""
import hashlib
import re

from .backends import Region

NODE_TYPES = {}


def register(cls):
    NODE_TYPES[cls.type_name] = cls
    return cls


class Node:
    type_name = "node"
    has_region = False

    def __init__(self, name, **config):
        self.name = name
        self.config = dict(config)
        self.region = Region() if self.has_region else None
        # overlay appearance (title bar-less window)
        self.color = "#00aaff"
        self.opacity = 0.3

    def process(self, inputs, backends):
        """Return output value, or None when the node produced nothing."""
        raise NotImplementedError

    def to_dict(self):
        d = {"type": self.type_name, "name": self.name, "config": self.config,
             "color": self.color, "opacity": self.opacity}
        if self.region:
            d["region"] = vars(self.region).copy()
        return d

    @staticmethod
    def from_dict(d):
        node = NODE_TYPES[d["type"]](d["name"], **d.get("config", {}))
        node.color = d.get("color", node.color)
        node.opacity = d.get("opacity", node.opacity)
        if node.has_region and "region" in d:
            node.region = Region(**d["region"])
        return node


@register
class OCRNode(Node):
    """Watches a region; emits text only when pixels changed since last refresh."""
    type_name = "ocr"
    has_region = True

    def __init__(self, name, **config):
        super().__init__(name, **config)
        self._last_hash = None

    def process(self, inputs, backends):
        image = backends.capture(self.region)
        digest = hashlib.md5(image.tobytes()).hexdigest()
        if digest == self._last_hash:
            return None
        self._last_hash = digest
        return backends.ocr(image)


@register
class ModelNode(Node):
    """Converts a value to a format: number, int, text, bool, json."""
    type_name = "model"

    def process(self, inputs, backends):
        if not inputs:
            return None
        value = inputs[0]
        fmt = self.config.get("format", "text")
        try:
            if fmt == "number":
                m = re.search(r"-?\d+(?:[.,]\d+)?", str(value))
                return float(m.group().replace(",", ".")) if m else None
            if fmt == "int":
                m = re.search(r"-?\d+", str(value))
                return int(m.group()) if m else None
            if fmt == "bool":
                return str(value).strip().lower() in ("1", "true", "yes", "on")
            if fmt == "json":
                import json
                return json.loads(str(value))
            return str(value).strip()
        except ValueError:
            return None


@register
class FunctionNode(Node):
    """Runs user code. `inputs` (list) is available; assign `result`.

    Returning None (result stays None) stops downstream action nodes.
    Example: ``result = inputs[0] > 10 and "go" or None``
    The code is user-authored and executed locally; only load trusted graphs.
    """
    type_name = "function"

    def process(self, inputs, backends):
        scope = {"inputs": inputs, "result": None}
        exec(self.config.get("code", "result = inputs[0] if inputs else None"),
             {"__builtins__": __builtins__}, scope)
        return scope["result"]


@register
class KeyboardNode(Node):
    type_name = "keyboard"
    has_region = True

    def process(self, inputs, backends):
        if not inputs or inputs[0] is None:
            return None
        text = self.config.get("text") or str(inputs[0])
        backends.click(*self.region.center())  # focus target region
        backends.type_text(text)
        return text


@register
class ClickNode(Node):
    type_name = "click"
    has_region = True

    def process(self, inputs, backends):
        if not inputs or inputs[0] is None:
            return None
        backends.click(*self.region.center(), button=self.config.get("button", "left"))
        return True
