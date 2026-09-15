"""插件的分帧要和 Rust 那一侧对得上。

两边各有一份 `Reassembler` 的实现，而**只有 Rust 那一侧有单元测试**。两边一旦
对不上，表现是插件从某一帧起再也收不到任何数据、整个天空清空——`can-audio`
就是这么坏过一次（v1 按字符串切片，切口落在汉字中间）。

金文件由 `crates/can-voice-sim/tests/bridge_golden.rs` 生成，里面是 Rust 编出来
的真包和它们该拼回的消息。跑法：

    python3 apps/xpc/plugin/test_bridge.py
"""

import importlib.util
import json
import pathlib
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
GOLDEN = HERE.parents[2] / "crates" / "can-voice-sim" / "testdata" / "bridge-golden.json"


def load_plugin():
    """把插件当普通模块导进来。

    它在 X-Plane 之外 `import xp` 会失败，文件里已经把那一句包在 try 里，
    所以这里直接导就行——那个 try 存在的理由正是这条测试。
    """
    spec = importlib.util.spec_from_file_location("pi_xpc_traffic", HERE / "PI_XpcTraffic.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class BridgeGoldenTest(unittest.TestCase):
    def setUp(self):
        self.plugin = load_plugin()
        self.golden = json.loads(GOLDEN.read_text(encoding="utf-8"))

    def test_every_case_reassembles_to_the_same_message(self):
        self.assertGreaterEqual(len(self.golden), 3, "金文件像是空的")
        for name, case in self.golden.items():
            with self.subTest(case=name):
                reassembler = self.plugin.Reassembler()
                got = None
                for packet in case["packets"]:
                    got = reassembler.feed(packet.encode("utf-8"))
                self.assertEqual(got, case["message"], f"{name} 拼不回原文")

    def test_a_cut_through_a_chinese_character_survives(self):
        """这一条是 v1 坏掉的那个具体形状。"""
        case = self.golden["chinese-cut-in-half"]
        self.assertGreater(len(case["packets"]), 10, "要切得够碎才测得到")
        reassembler = self.plugin.Reassembler()
        got = None
        for packet in case["packets"]:
            got = reassembler.feed(packet.encode("utf-8"))
        self.assertEqual(got, case["message"])

    def test_the_protocol_version_matches(self):
        """版本对不上时插件会**静默地**丢掉每一个包。"""
        first = json.loads(self.golden["small"]["packets"][0])
        self.assertEqual(first["v"], self.plugin.PROTOCOL_VERSION)


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False).result.wasSuccessful() else 1)
