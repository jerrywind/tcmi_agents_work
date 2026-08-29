"""典籍召回质量评估（T4.3）。

设计要点：

- **离线可跑**：直接查 `corpus.CorpusIndex`（SQLite），不依赖 Embedding 端点，
  因此 CI 与本地都能复现同一份分数；
- **判据以「内容」为准，而不是「书名」**：语料有 694 部，同一张经方在
  《伤寒论》《医宗金鉴》《证治准绳》《类证治裁》里都有论述，
  「应该出自哪本书」本身没有唯一答案——按书名打分会把正确答案判错。
  真正要保证的是：**召回到能回答这个问题的那段原文**。故主判据是
  1. `expect_quote`——应当出现的**原文字样**（如「半夏泻心汤」），
     命中即说明找对了段落，据此算 hit@k 与 MRR；
  2. `expect_terms`——top-k 里应当覆盖的关键词，衡量上下文是否完整。
  `expect_books` 只作为**提示**输出（人工复核用），不计分。

样例集 `eval/tcm_queries.jsonl` 的取值原则：
每条的 quote 与 terms 都必须是**语料中确实存在**的用词，
否则低分只说明样例出错了，不说明检索差。
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass, field
from pathlib import Path

try:
    from .corpus import CorpusIndex
except ImportError:  # 作为脚本直接运行时退化为绝对导入
    from corpus import CorpusIndex


@dataclass
class EvalCase:
    """一条评估样例。"""

    query: str
    # 应当被召回的**原文字样**：命中即认为找对了段落
    expect_quote: str = ""
    expect_terms: list[str] = field(default_factory=list)
    # 仅作提示（人工复核用）：同一知识在多部典籍中都有论述，不按书名计分
    expect_books: list[str] = field(default_factory=list)
    note: str = ""


@dataclass
class CaseResult:
    query: str
    quote_hit: bool
    quote_rank: int          # 0 表示未命中
    term_coverage: float
    reciprocal_rank: float
    latency_ms: float
    top_sources: list[str]
    missing_terms: list[str]


def load_cases(path: Path) -> list[EvalCase]:
    cases: list[EvalCase] = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        d = json.loads(line)
        cases.append(
            EvalCase(
                query=d["query"],
                expect_quote=d.get("expect_quote", ""),
                expect_terms=d.get("expect_terms", []),
                expect_books=d.get("expect_books", []),
                note=d.get("note", ""),
            )
        )
    return cases


def run_case(idx: CorpusIndex, case: EvalCase, *, top_k: int, top_docs: int) -> CaseResult:
    t0 = time.perf_counter()
    hits = idx.search(case.query, top_k=top_k, top_docs=top_docs)
    latency_ms = (time.perf_counter() - t0) * 1000.0

    # 命中判据：哪一条片段里出现了应当出现的原文字样
    rank = 0
    for i, h in enumerate(hits, 1):
        if case.expect_quote and case.expect_quote in h.text:
            rank = i
            break
    rr = 1.0 / rank if rank else 0.0

    blob = "\n".join(h.text for h in hits)
    found = [t for t in case.expect_terms if t in blob]
    coverage = (len(found) / len(case.expect_terms)) if case.expect_terms else 1.0

    return CaseResult(
        query=case.query,
        quote_hit=rank > 0,
        quote_rank=rank,
        term_coverage=coverage,
        reciprocal_rank=rr,
        latency_ms=round(latency_ms, 1),
        top_sources=[f"《{h.book}》{h.meta.get('section', '')}".rstrip() for h in hits],
        missing_terms=[t for t in case.expect_terms if t not in found],
    )


def run_eval(queries_path: Path, db_path: Path, *, top_k: int = 5,
             top_docs: int = 3) -> dict:
    """跑完整评估，返回 {"text": 人类可读报告, "json": 结构化结果}。"""
    cases = load_cases(Path(queries_path))
    if not Path(db_path).exists():
        raise SystemExit(f"典籍索引不存在：{db_path}（先跑 `python -m rag corpus-build`）")

    idx = CorpusIndex(db_path)
    try:
        stats = idx.stats()
        results = [run_case(idx, c, top_k=top_k, top_docs=top_docs) for c in cases]
    finally:
        idx.close()

    n = len(results) or 1
    hit_at_k = sum(1 for r in results if r.quote_hit) / n
    hit_at_1 = sum(1 for r in results if r.quote_rank == 1) / n
    coverage = sum(r.term_coverage for r in results) / n
    mrr = sum(r.reciprocal_rank for r in results) / n
    avg_ms = sum(r.latency_ms for r in results) / n
    p95_ms = sorted(r.latency_ms for r in results)[min(n - 1, int(n * 0.95))]

    lines = [
        f"典籍召回评估：{len(results)} 条查询 / top_k={top_k} / top_docs={top_docs}",
        f"索引：{stats['books']} 部 / {stats['chars']:,} 字 / posting {stats['postings']:,}",
        "",
        f"原文命中率 hit@{top_k}  : {hit_at_k:.2%}",
        f"首条命中率 hit@1       : {hit_at_1:.2%}",
        f"关键词覆盖率           : {coverage:.2%}",
        f"MRR                    : {mrr:.3f}",
        f"耗时 均值/p95          : {avg_ms:.0f} / {p95_ms:.0f} ms",
        "",
    ]
    for r in results:
        flag = "OK  " if r.quote_hit else "MISS"
        where = r.top_sources[0] if r.top_sources else "无命中"
        lines.append(
            f"[{flag}] {r.query}  → 第{r.quote_rank or '-'}条命中，首条出自 {where}"
            f"  覆盖率={r.term_coverage:.0%}  用时={r.latency_ms:.0f}ms"
        )
        if r.missing_terms:
            lines.append(f"        缺失关键词：{'、'.join(r.missing_terms)}")

    return {
        "text": "\n".join(lines),
        "json": {
            "top_k": top_k,
            "top_docs": top_docs,
            "cases": len(results),
            "hit_at_k": round(hit_at_k, 4),
            "hit_at_1": round(hit_at_1, 4),
            "term_coverage": round(coverage, 4),
            "mrr": round(mrr, 4),
            "avg_latency_ms": round(avg_ms, 1),
            "p95_latency_ms": round(p95_ms, 1),
            "index": stats,
            "details": [r.__dict__ for r in results],
        },
    }


if __name__ == "__main__":  # pragma: no cover
    import argparse

    ap = argparse.ArgumentParser()
    ap.add_argument("--queries", default="eval/tcm_queries.jsonl")
    ap.add_argument("--db", default="/data/rag/corpus.sqlite3")
    ap.add_argument("--top-k", type=int, default=5)
    ap.add_argument("--top-docs", type=int, default=3)
    a = ap.parse_args()
    print(run_eval(Path(a.queries), Path(a.db), top_k=a.top_k, top_docs=a.top_docs)["text"])
