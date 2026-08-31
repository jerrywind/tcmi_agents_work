"""中医典籍多标签分类的离线测试。

规则分类最容易出的是**静默误判**：书名里碰巧有个「方」字就被当成方书，
语料里作者字段张冠李戴就把《伤科补要》挂上「伤寒派」。这些用例按真实踩过的
坑逐个钉死，全部不联网、不依赖 Embedding 端点。
"""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

try:
    from .corpus import CorpusIndex
    from .taxonomy import (
        DEPARTMENT, GENRE, FUNCTION, SCHOOL, DIMENSIONS, FALLBACK_TAG,
        build_report, classify, classify_corpus, count_tags,
        infer_era, parse_meta, read_head, scan_corpus,
    )
except ImportError:  # 与 test_rag.py 一致：直接 `python -m unittest test_taxonomy` 时走平铺导入
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from corpus import CorpusIndex  # noqa: E402
    from taxonomy import (  # noqa: E402
        DEPARTMENT, GENRE, FUNCTION, SCHOOL, DIMENSIONS, FALLBACK_TAG,
        build_report, classify, classify_corpus, count_tags,
        infer_era, parse_meta, read_head, scan_corpus,
    )

# 现场构造的最小语料：一部儿科书 + 一部本草书
PED_BOOK = """<篇名>测试幼科
书名：测试幼科
作者：钱乙
朝代：宋
年份：未知

<目录>
<篇名>咳嗽
内容：一小儿伤食，发热抽搐，呕吐喘嗽，用六君桔梗桑皮杏仁治之。
<篇名>痘疹
内容：小儿痘疹发热，宜用升麻葛根汤，不可妄投寒凉。
"""

HERB_BOOK = """<篇名>测试本草
书名：测试本草
作者：李时珍
朝代：明

<目录>
<篇名>附子
内容：附子味辛温，主风寒咳逆邪气，温中，金疮，破癥坚积聚。
<篇名>干姜
内容：干姜味辛温，主胸满咳逆上气，温中，止血，出汗，逐风湿痹。
"""


class TestClassify(unittest.TestCase):
    def test_four_dimensions(self):
        """四维彼此正交，一部书每个维度都有值（打不上就是 `未归类`）。"""
        tags = classify("本草纲目", "李时珍")
        self.assertEqual(sorted(tags), sorted(DIMENSIONS))
        self.assertEqual(list(tags), list(DIMENSIONS))  # 维度顺序也要固定
        self.assertIn("本草方药", tags[DEPARTMENT])
        self.assertIn("本草药物", tags[GENRE])
        self.assertIn("本草集解", tags[FUNCTION])
        self.assertEqual(tags[SCHOOL], [FALLBACK_TAG])

    def test_multi_label(self):
        """《妇人大全良方》既是妇科也是产科；《范中林六经辨证医案》兼跨两派。"""
        fu = classify("妇人大全良方", "陈自明")
        self.assertIn("妇科", fu[DEPARTMENT])
        self.assertIn("产科", fu[DEPARTMENT])
        fan = classify("范中林六经辨证医案", "范中林")
        self.assertEqual(set(fan[SCHOOL]), {"伤寒派", "火神派"})

    def test_traditional_chinese_title(self):
        """繁体书名要先转简体再匹配，否则整条规则漏掉。"""
        tags = classify("十二經補瀉溫涼引經藥歌", "")
        self.assertIn("本草药物", tags[GENRE])
        # 「引经」是药物归经，不是针灸经络
        self.assertNotIn("针灸经络", tags[GENRE])

    def test_author_decides_school(self):
        """流派看作者：郑钦安 → 火神派，叶天士 → 温热派。"""
        self.assertIn("火神派", classify("医理真传", "郑钦安")[SCHOOL])
        self.assertIn("温热派", classify("临证指南医案", "叶桂")[SCHOOL])

    def test_author_rule_skips_specialty_titles(self):
        """语料里《伤科补要》的作者被错标成钱潢（伤寒大家），
        书名带「伤科」时必须无视作者，否则会挂上错误的流派。"""
        self.assertEqual(classify("伤科补要", "钱潢")[SCHOOL], [FALLBACK_TAG])

    def test_genre_falls_back_to_department(self):
        """方书/医案本身不分科，书名里没有科室线索时应落到「全科综合」。"""
        self.assertIn("全科综合", classify("医方考", "吴昆")[DEPARTMENT])
        self.assertIn("全科综合", classify("洄溪医案", "")[DEPARTMENT])

    def test_specialty_book_keeps_its_department(self):
        """专科专著不该被「全科综合」吞掉。"""
        self.assertIn("儿科", classify("幼科铁镜", "夏禹铸")[DEPARTMENT])
        self.assertIn("眼科", classify("审视瑶函", "傅仁宇")[DEPARTMENT])
        self.assertIn("产科", classify("胎产心法", "闵纯玺")[DEPARTMENT])

    def test_jin_kui_is_not_formulary(self):
        """《金匮要略方论》含「方」字，但它是仲景经典而非方书汇编。"""
        self.assertNotIn("方书方剂", classify("金匮要略方论", "张仲景")[GENRE])

    def test_qian_hu_is_not_herbal(self):
        """「濒湖」是李时珍的号：《濒湖脉学》是脉学书，不能因号入本草。"""
        tags = classify("濒湖脉学", "李时珍")
        self.assertNotIn("本草药物", tags[GENRE])
        self.assertEqual(tags[FUNCTION], ["诊断方法"])


class TestFunction(unittest.TestCase):
    """`功能用途` 是 `内容体裁` 的细分：同是本草书，查药性和查炮制是两回事。"""

    def test_herbal_by_function(self):
        cases = {
            "本草纲目": "本草集解",
            "雷公炮炙论": "炮制制剂",
            "食疗本草": "食疗本草",
            "本草图经": "药物图谱",
            "汤液本草": "药性理论",
            "得配本草": "配伍归经",
            "神农本草经": "经典本草",
            "本草备要": "本草入门",
        }
        for title, fn in cases.items():
            self.assertIn(fn, classify(title)[FUNCTION], title)

    def test_formulary_by_function(self):
        cases = {
            "太平惠民和剂局方": "成药标准",
            "医方考": "方论阐释",
            "汤头歌诀": "歌诀便诵",
            "验方新编": "经验验方",
            "妇人大全良方": "专科方书",
        }
        for title, fn in cases.items():
            self.assertIn(fn, classify(title)[FUNCTION], title)

    def test_acupuncture_by_function(self):
        cases = {
            "子午流注针经": "时间针法",
            "厘正按摩要术": "推拿按摩",
            "十四经发挥": "经络理论",
            "宋本备急灸法": "刺法灸法",
            "经穴汇解": "腧穴考证",
            "针灸大成": "针灸综合",
        }
        for title, fn in cases.items():
            self.assertIn(fn, classify(title)[FUNCTION], title)

    def test_function_falls_back_from_genre(self):
        """没有特殊功能的按体裁兜底：《伤寒论》是经典，《临证指南医案》是实录。"""
        self.assertEqual(classify("伤寒论", "张仲景")[FUNCTION], ["经典诠释"])
        self.assertEqual(classify("临证指南医案", "叶桂")[FUNCTION], ["临证实录"])

    def test_bei_ji_is_not_first_aid(self):
        """「备急」不能单独作急救判据：《备急千金要方》是综合方书。"""
        self.assertIn("急救方书", classify("肘后备急方", "葛洪")[FUNCTION])
        self.assertNotIn("急救方书", classify("备急千金要方", "孙思邈")[FUNCTION])

    def test_also_of_blocks_partial_match(self):
        """`also_of` 生效：带专科词但不带「方」的《外科正宗》《外科证治全书》
        不是方书；带「撮要」但无本草味的《绛囊撮要》也不是本草入门。"""
        self.assertNotIn("专科方书", classify("外科正宗", "陈实功")[FUNCTION])
        self.assertNotIn("本草入门", classify("绛囊撮要")[FUNCTION])

    def test_twelve_meridians_is_not_meridian_theory(self):
        """《十二经补泻温凉引经药歌》讲的是药物归经，不是经络理论。"""
        tags = classify("十二經補瀉溫涼引經藥歌")
        self.assertIn("配伍归经", tags[FUNCTION])
        self.assertNotIn("经络理论", tags[FUNCTION])

    def test_person_name_fang_is_not_formulary(self):
        """《西方子明堂灸经》里的「方」是人名用字，不是方书。"""
        tags = classify("西方子明堂灸经")
        self.assertNotIn("方书方剂", tags[GENRE])
        self.assertIn("刺法灸法", tags[FUNCTION])

    def test_acupuncture_volume_of_pu_ji_fang(self):
        """《普济方·针灸》体裁挂着「临床综合」，功能是针灸而非综合证治。"""
        self.assertEqual(classify("普济方·针灸")[FUNCTION], ["针灸综合"])


class TestMeta(unittest.TestCase):
    def test_parse_meta(self):
        head = "<篇名>伤寒论\n书名：伤寒论\n作者：张仲景\n朝代：东汉\n年份：公元25-220年\n"
        meta = parse_meta(head)
        self.assertEqual(meta["author"], "张仲景")
        self.assertEqual(meta["dynasty"], "东汉")

    def test_parse_meta_loose_author(self):
        """「原著 清·郑钦安」这种非标准写法也要能抽出来。"""
        self.assertEqual(parse_meta("中医瑰宝苑\n原著 清·郑钦安\n\n")["author"], "郑钦安")

    def test_infer_era(self):
        self.assertEqual(infer_era("东汉"), "先秦两汉")
        self.assertEqual(infer_era("宋"), "宋金元")
        self.assertEqual(infer_era("明·万历六年"), "明清")
        self.assertEqual(infer_era("民国"), "近现代")
        self.assertEqual(infer_era(""), "")

    def test_read_head_survives_truncated_multibyte(self):
        """截断正好落在汉字中间时不能静默吞字（`errors="ignore"` 的坑）。"""
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "h.txt"
            p.write_bytes("书名：测试本草\r\n作者：李时珍\r\n".encode("gb18030") * 50)
            head = read_head(p, limit=37)  # 故意切在多字节汉字中间
        self.assertIn("书名", head)
        self.assertTrue(head)


class TestCorpusClassification(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory()
        cls.dir = Path(cls._tmp.name) / "corpus"
        cls.dir.mkdir()
        (cls.dir / "001-测试幼科.txt").write_bytes(PED_BOOK.encode("gb18030"))
        (cls.dir / "002-测试本草.txt").write_bytes(HERB_BOOK.encode("gb18030"))
        cls.db = Path(cls._tmp.name) / "corpus.sqlite3"

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def test_scan_and_count(self):
        books = list(scan_corpus(self.dir))
        self.assertEqual(len(books), 2)
        by_id = {b.doc_id: b for b in books}
        self.assertEqual(by_id["001"].author, "钱乙")
        self.assertEqual(by_id["001"].era, "宋金元")
        self.assertIn("儿科", by_id["001"].tags[DEPARTMENT])
        self.assertIn("本草药物", by_id["002"].tags[GENRE])
        counts = count_tags(books)
        self.assertEqual(counts[DEPARTMENT]["儿科"], 1)

    def test_report_contains_every_tag(self):
        books = list(scan_corpus(self.dir))
        md = build_report(books, count_tags(books), self.dir)
        self.assertIn("临床学科", md)
        self.assertIn("本草药物", md)
        self.assertIn("《测试幼科》", md)

    def test_classify_corpus_writes_json(self):
        out = Path(self._tmp.name) / "classification.json"
        result = classify_corpus(self.dir, json_path=out)
        self.assertEqual(result["books"], 2)
        self.assertTrue(out.exists())
        self.assertEqual(len(result["books_detail"]), 2)

    def test_tags_roundtrip_and_filtered_search(self):
        idx = CorpusIndex(self.db)
        idx.build(self.dir, min_tf=1)
        books = list(scan_corpus(self.dir))
        idx.write_classification(books)

        self.assertEqual(idx.tag_counts(DEPARTMENT)["儿科"], 1)
        # 只在儿科书里搜：本草书里也有「咳逆」，不该混进来
        hits = idx.search("小儿发热咳嗽", top_k=3, tags=["儿科"])
        self.assertTrue(hits)
        self.assertTrue(all(h.book == "测试幼科" for h in hits))
        # 不加标签时同一查询会跨书命中
        self.assertTrue(idx.search("附子干姜", top_k=3))
        idx.close()


class TestTagGuard(unittest.TestCase):
    def test_search_with_tags_on_unclassified_db_raises(self):
        """没跑过分类就按标签过滤，必须显式报错而不是静默返回空。"""
        with tempfile.TemporaryDirectory() as d:
            src = Path(d) / "corpus"
            src.mkdir()
            (src / "001-测试幼科.txt").write_bytes(PED_BOOK.encode("gb18030"))
            idx = CorpusIndex(Path(d) / "db.sqlite3")
            idx.build(src, min_tf=1)
            with self.assertRaises(ValueError):
                idx.doc_ords_for_tags(["儿科"])
            idx.close()


if __name__ == "__main__":
    unittest.main()
