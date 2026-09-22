"""Exercise the real text filter without loading Whisper or starting the server."""
import ast
from pathlib import Path
import re
import unittest


source = Path(__file__).resolve().parents[1] / "src-tauri/resources/stt-server.py"
tree = ast.parse(source.read_text())
nodes = [node for node in tree.body if (
    isinstance(node, ast.FunctionDef) and node.name == "_is_hallucination"
) or (
    isinstance(node, ast.Assign) and any(
        isinstance(target, ast.Name) and target.id.startswith("_HALLU")
        for target in node.targets
    )
)]
namespace = {"re": re}
exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), "exec"), namespace)
is_hallucination = namespace["_is_hallucination"]


class HallucinationFilterTests(unittest.TestCase):
    def test_reported_pages_instruction(self):
        self.assertFalse(is_hallucination(
            "没有在这个pages里面已经有你之前粘贴进去的conclusion了,然后我就是要你把刚刚你粘贴进去的那些内容全部翻译成中文,放在最下面。"
        ))

    def test_normal_requests_and_quoted_boilerplate(self):
        for text in (
            "请帮我翻译这篇文章。", "翻译", "打开字幕", "查看订阅费用",
            "这首歌是谁作词作曲的？", "帮我查版权信息", "请关注这段文字的语气",
            "在结尾写上感谢观看。", "请翻译：thank you for watching",
            "Translate the sentence translated by John into Chinese.",
            "感谢观看，然后帮我打开文档。", "请把以下是普通话的句子翻译成英文",
        ):
            with self.subTest(text=text):
                self.assertFalse(is_hallucination(text))

    def test_known_standalone_boilerplate(self):
        for text in (
            "感谢观看！", "谢谢收看，请订阅。", "Thank you for watching!",
            "Please subscribe.", "以下是普通话的句子。", "以下是繁體中文的句子。",
            "请不吝点赞订阅转发打赏支持明镜与点点栏目",
        ):
            with self.subTest(text=text):
                self.assertTrue(is_hallucination(text))

    def test_existing_noise_filters(self):
        for text in ("", " ", "...", "♪♫", "啊啊啊啊啊", "00:12:30", "我会说,我会说,我会说,我会说"):
            with self.subTest(text=text):
                self.assertTrue(is_hallucination(text))


if __name__ == "__main__":
    unittest.main()
