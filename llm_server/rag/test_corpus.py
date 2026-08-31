"""典籍语料索引（T4.3）的离线测试。

全部用例不依赖网络与 Embedding 端点：索引只用到标准库（sqlite3）。
语料文本在临时目录里现场构造，包括**故意用 GB18030 编码**的文件——
真实语料就是这个编码，曾导致「索引建好了却什么都搜不到」，必须回归。
"""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

try:
    from .corpus import (
        CorpusIndex,
        bigrams,
        chunk_text,
        count_bigrams,
        iter_books,
        normalize,
        parse_sections,
        read_text,
    )
    from .eval_rag import run_eval
    from .taxonomy import scan_corpus
except ImportError:  # 与 test_rag.py 一致：直接 `python -m unittest test_corpus` 时退化为平铺导入
    import sys
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from corpus import (  # noqa: E402
        CorpusIndex,
        bigrams,
        chunk_text,
        count_bigrams,
        iter_books,
        normalize,
        parse_sections,
        read_text,
    )
    from eval_rag import run_eval  # noqa: E402
    from taxonomy import scan_corpus  # noqa: E402

# 片段正文（GB18030 写出，测试里再读回来）
BOOK_A = """<篇名>测试本草
书名：测试本草
作者：某人
朝代：清
年份：未知

<目录>
<篇名>甘草
内容：甘草味甘平。主五脏六腑寒热邪气，坚筋骨，长肌肉，倍气力，金疮肿，解毒。
<篇名>麻黄
内容：麻黄味苦温。主中风伤寒头痛，温疟，发表出汗，去邪热气，止咳逆上气。
"""

BOOK_B = """<篇名>测试方书
书名：测试方书
作者：另一人

<目录>
<篇名>桂枝汤
内容：太阳中风，阳浮而阴弱。啬啬恶寒，淅淅恶风，翕翕发热，鼻鸣干呕者，桂枝汤主之。
"""

# 儿科方书：既是「儿科」又是「方书方剂」，用来验证跨维度交集
BOOK_C = """<篇名>测试幼科方
书名：测试幼科方
作者：第三人

<目录>
<篇名>小儿咳嗽
内容：小儿发热咳嗽，宜用杏苏散加减。
<篇名>小儿痘疹
内容：小儿痘疹初起，发热咳嗽，宜升麻葛根汤。
"""


class TestTagFiltering(unittest.TestCase):
    """按标签收窄检索范围。

    四个分类维度正交，「儿科的方书」是两个维度的**交集**——扁平并集会把
    「儿科的医案」和「内科的方书」一起捞进来，那不是调用方想要的。
    """

    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory()
        cls.dir = Path(cls._tmp.name) / "corpus"
        cls.dir.mkdir()
        # 儿科方书：既是儿科又是方书
        (cls.dir / "001-测试幼科方.txt").write_bytes(BOOK_C.encode("gb18030"))
        # 本草书：既不是儿科也不是方书
        (cls.dir / "002-测试本草.txt").write_bytes(BOOK_A.encode("gb18030"))
        cls.db = Path(cls._tmp.name) / "corpus.sqlite3"
        idx = CorpusIndex(cls.db)
        idx.build(cls.dir, min_tf=1)
        idx.write_classification(list(scan_corpus(cls.dir)))
        cls.idx = idx

    @classmethod
    def tearDownClass(cls):
        cls.idx.close()
        cls._tmp.cleanup()

    def test_flat_tags_are_union(self):
        """扁平 `tags` 是并集：儿科 OR 方书 -> 两本都进。"""
        ords = self.idx.doc_ords_for_tags(["儿科", "本草药物"])
        self.assertEqual(len(ords), 2)

    def test_tag_groups_are_intersection(self):
        """`tag_groups` 组内并集、组间交集。"""
        only_ped = self.idx.doc_ords_for_tags(groups=[["儿科"]])
        self.assertEqual(len(only_ped), 1)
        # 儿科 AND 方书方剂 -> 只有《测试幼科方》
        both = self.idx.doc_ords_for_tags(groups=[["儿科"], ["方书方剂"]])
        self.assertEqual(both, only_ped)
        # 儿科 AND 本草药物 -> 空（没有既是儿科又是本草的书）
        self.assertEqual(self.idx.doc_ords_for_tags(
            groups=[["儿科"], ["本草药物"]]), set())

    def test_search_with_tag_groups(self):
        hits = self.idx.search("发热咳嗽", top_k=3, tag_groups=[["儿科"], ["方书方剂"]])
        self.assertTrue(hits)
        self.assertTrue(all(h.book == "测试幼科方" for h in hits))

    def test_unclassified_db_raises_instead_of_silent_empty(self):
        """没跑过分类就按标签过滤，必须显式报错而不是静默返回空。"""
        with tempfile.TemporaryDirectory() as d:
            src = Path(d) / "corpus"
            src.mkdir()
            (src / "001-测试幼科方.txt").write_bytes(BOOK_C.encode("gb18030"))
            idx = CorpusIndex(Path(d) / "db.sqlite3")
            idx.build(src, min_tf=1)
            with self.assertRaises(ValueError):
                idx.doc_ords_for_tags(groups=[["儿科"]])
            idx.close()


class TestTextUtils(unittest.TestCase):
    def test_normalize_keeps_only_cjk(self):
        self.assertEqual(normalize("甘草，味甘平。A1２３"), "甘草味甘平")

    def test_count_bigrams(self):
        # 「甘草甘草」的 bigram：甘草、草甘、甘草
        c = count_bigrams("甘草甘草")
        self.assertEqual(c["甘草"], 2)
        self.assertEqual(c["草甘"], 1)
        # 键必须是字符串，与 bigrams() 的输出可直接比较
        self.assertEqual(set(c), set(bigrams("甘草甘草")))
        self.assertTrue(all(isinstance(k, str) for k in c))

    def test_chunk_text_merges_and_splits(self):
        # 短段落会被合并成一条；过短的片段（< 8 字）按噪声丢弃
        short = chunk_text("一二三四五六七八九十。\n一二三四五六七八九十。\n", max_chars=50)
        self.assertEqual(len(short), 1)
        self.assertEqual(chunk_text("短。"), [], "过短片段应丢弃")
        long_text = "甲乙丙丁" * 100
        chunks = chunk_text(long_text, max_chars=100, overlap=20)
        self.assertTrue(len(chunks) > 1)
        self.assertTrue(all(len(c) <= 100 for c in chunks))

    def test_parse_sections_drops_metadata_header(self):
        sections = parse_sections(BOOK_A)
        names = [s for s, _ in sections]
        self.assertNotIn("测试本草", names, "书目元数据块（书名/作者/朝代）应被剔除")
        self.assertIn("甘草", names)
        self.assertIn("麻黄", names)
        body = dict(sections)["甘草"]
        self.assertNotIn("内容：", body, "正文前缀标签应被去掉")
        self.assertIn("主五脏六腑寒热邪气", body)


class TestEncoding(unittest.TestCase):
    def test_read_text_detects_gb18030(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "book.txt"
            p.write_bytes(BOOK_A.encode("gb18030"))
            text = read_text(p)
            self.assertIn("甘草", text)
            self.assertIn("主五脏六腑寒热邪气", text)
            # 汉字总数应与原文本一致（即没有因编码错误被丢弃）
            self.assertEqual(len(normalize(text)), len(normalize(BOOK_A)))

    def test_read_text_still_reads_utf8(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "book.txt"
            p.write_text(BOOK_A, encoding="utf-8")
            self.assertIn("甘草", read_text(p))


class TestIndex(unittest.TestCase):
    def setUp(self) -> None:
        self._dir = tempfile.TemporaryDirectory()
        root = Path(self._dir.name)
        self.corpus = root / "corpus"
        self.corpus.mkdir()
        # 真实语料就是 GB18030：这里刻意沿用，防止回归
        (self.corpus / "000-测试本草.txt").write_bytes(BOOK_A.encode("gb18030"))
        (self.corpus / "001-测试方书.txt").write_bytes(BOOK_B.encode("gb18030"))
        self.db = root / "index.sqlite3"

    def tearDown(self) -> None:
        self._dir.cleanup()

    def test_iter_books_parses_no_and_title(self):
        books = list(iter_books(self.corpus))
        self.assertEqual([b.doc_id for b in books], ["000", "001"])
        self.assertEqual([b.title for b in books], ["测试本草", "测试方书"])

    def test_build_and_search(self):
        with CorpusIndex(self.db) as idx:
            stat = idx.build(self.corpus)
            self.assertEqual(stat["books"], 2)

            hits = idx.search("甘草 味甘平 主五脏六腑寒热邪气", top_k=3)
            self.assertTrue(hits, "应能检索到内容")
            self.assertEqual(hits[0].book, "测试本草")
            self.assertIn("甘草", hits[0].text)
            self.assertEqual(hits[0].meta.get("section"), "甘草")

            hits2 = idx.search("太阳中风 恶风 桂枝汤主之", top_k=3)
            self.assertEqual(hits2[0].book, "测试方书")

    def test_search_returns_empty_for_unknown_terms(self):
        with CorpusIndex(self.db) as idx:
            idx.build(self.corpus)
            self.assertEqual(idx.search("量子纠缠超导"), [])

    def test_per_doc_limit_prevents_one_book_monopoly(self):
        with CorpusIndex(self.db) as idx:
            idx.build(self.corpus)
            hits = idx.search("内容 主之 味甘", top_k=4, top_docs=2, per_doc=1)
            books = [h.book for h in hits]
            self.assertEqual(len(books), len(set(books)), f"每部书最多 1 条：{books}")

    def test_stats(self):
        with CorpusIndex(self.db) as idx:
            idx.build(self.corpus)
            s = idx.stats()
            self.assertEqual(s["books"], 2)
            self.assertTrue(s["postings"] > 0)
            self.assertTrue(s["chars"] > 0)


class TestEval(unittest.TestCase):
    def test_eval_runs_offline_and_reports_metrics(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            corpus = root / "corpus"
            corpus.mkdir()
            (corpus / "000-测试本草.txt").write_bytes(BOOK_A.encode("gb18030"))
            db = root / "index.sqlite3"
            with CorpusIndex(db) as idx:
                idx.build(corpus)

            qf = root / "queries.jsonl"
            qf.write_text(
                json.dumps({"query": "甘草 五脏六腑寒热邪气", "expect_quote": "甘草",
                            "expect_terms": ["甘草"]}, ensure_ascii=False)
                + "\n"
                + json.dumps({"query": "不存在的病名术语", "expect_quote": "绝对没有",
                              "expect_terms": []}, ensure_ascii=False)
                + "\n",
                encoding="utf-8",
            )
            report = run_eval(qf, db, top_k=3, top_docs=2)
            self.assertEqual(report["json"]["cases"], 2)
            # 一条能命中、一条必然落空
            self.assertAlmostEqual(report["json"]["hit_at_k"], 0.5)
            self.assertAlmostEqual(report["json"]["hit_at_1"], 0.5)


if __name__ == "__main__":
    unittest.main()
