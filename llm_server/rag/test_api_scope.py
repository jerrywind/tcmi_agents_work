"""`/rag/retrieve/scope` 的知识域编译（sub-agent 检索域）测试。

靠真实语料库跑很慢且依赖索引存在，故这里只测**编译语义**——
「四维 scope 怎么变成标签过滤条件」正是 sub-agent 检索准不准的关键，
必须能脱离索引单独验证。
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

try:
    from .api import ScopeReq, scope_to_tag_groups
except ImportError:  # 与 test_rag.py 一致：直接 `python -m unittest` 时走平铺导入
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from api import ScopeReq, scope_to_tag_groups  # noqa: E402


class TestScopeToTagGroups(unittest.TestCase):
    def test_cross_dimension_is_intersection(self):
        """体裁 AND 科室 —— 「儿科的方书」，不是「儿科或方书」。"""
        groups, flat = scope_to_tag_groups(ScopeReq(
            query="q", genres=["方书方剂"], departments=["儿科"]))
        self.assertEqual(groups, [["方书方剂"], ["儿科"]])
        self.assertEqual(flat, [])

    def test_within_dimension_is_union(self):
        """同一维度内是并集：「儿科 OR 产科」。"""
        groups, _ = scope_to_tag_groups(ScopeReq(
            query="q", genres=["方书方剂"], departments=["儿科", "产科"]))
        self.assertEqual(groups, [["方书方剂"], ["儿科", "产科"]])

    def test_empty_dimensions_are_ignored(self):
        """留空的维度不参与过滤，否则会生成空组把结果全砍掉。"""
        groups, flat = scope_to_tag_groups(ScopeReq(query="q", genres=["方书方剂"]))
        self.assertEqual(groups, [["方书方剂"]])
        self.assertEqual(flat, [])
        self.assertEqual(scope_to_tag_groups(ScopeReq(query="q")), ([], []))

    def test_require_all_false_flattens(self):
        """`require_all=false` 退回全部并集。"""
        groups, flat = scope_to_tag_groups(ScopeReq(
            query="q", genres=["方书方剂"], departments=["儿科"],
            require_all=False))
        self.assertEqual(groups, [])
        self.assertEqual(flat, ["方书方剂", "儿科"])

    def test_four_dimensions_together(self):
        """四维齐上：开方 agent 在「骨伤科 + 火神派」语境下查针灸刺法。"""
        groups, _ = scope_to_tag_groups(ScopeReq(
            query="q", genres=["针灸经络"], functions=["刺法灸法"],
            departments=["骨伤科"], schools=["火神派"]))
        self.assertEqual(groups, [["针灸经络"], ["刺法灸法"],
                                  ["骨伤科"], ["火神派"]])


if __name__ == "__main__":
    unittest.main()
