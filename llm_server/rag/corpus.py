"""中医典籍语料索引（T4.3）：两级检索，离线可用。

语料是 694 部中医典籍、约 161 MB 纯文本。把它按切片（~600 字）全量建倒排
会产生上亿条 posting——纯 Python + 单机下既不现实也不必要。故采用**两级检索**：

1. **书级倒排**（SQLite，标准库自带）：中文按**二元切分**（bigram）建立
   `postings(term, doc, tf)`，每部书只保留高频且相对有区分度的 term
   （`tf >= min_tf`，取 tf 最高的 `max_terms_per_doc` 个）。查询先被收敛到
   少数几部最相关的书；
2. **片段级精排**：只把命中书目的正文读入内存并切窗，用 bigram 覆盖率
   结合 IDF 加权打分，返回 top_k 个片段。

由此带来的性质：

- 索引体积与单次查询耗时都只与「命中书的大小」成正比，而不是与全库成正比；
- **不需要 Embedding 端点即可工作**（LM Studio 未加载 embedding 模型、
  端点不可达时依旧能检索），向量检索仍可与本索引并行、互为补充；
- 只用标准库（`sqlite3` / `re` / `collections`），不引入新依赖。

为什么用 bigram 而不是分词：中医典籍里有大量专有名词与古汉语表达，
通用分词器切不准；bigram 对未登录词鲁棒，且是 CJK 检索的经典做法。
"""

from __future__ import annotations

import math
import re
import sqlite3
import threading
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable, Iterator, Sequence

# 只保留汉字：标点、空白、拉丁字母与数字对中医典籍检索贡献极小，
# 且会显著放大词表与 posting 数量。
# 用转义写法而非字面量，避免源文件编码异常时把这条正则写坏。
_NON_CJK_RE = re.compile(r"[^一-鿿]+")
# 书名形如 `013-本草纲目.txt`：序号 + 书名
_BOOK_NAME_RE = re.compile(r"^(?P<no>\d+)[-_](?P<title>.+)$")
# 章节标记：`<篇名>辨太阳病脉证并治上`
_SECTION_RE = re.compile(r"^<篇名>(?P<name>.+)$", re.MULTILINE)
# 章节正文前缀标签
_BODY_PREFIX_RE = re.compile(r"^(?:内容|属性|注释)\s*[：:]\s*")
# 查询里的「整词」分隔符（空格、中英文逗号/顿号/分号）
_PHRASE_SPLIT_RE = re.compile(r"[\s,，、;；:：]+")
# 正文里夹带的导航标记（`<目录>卷之四十一\…`），对检索是噪声
_TOC_INLINE_RE = re.compile(r"<目录>.*")

# 语料实际编码是 **GB18030**（中医典籍站的常见导出格式），不是 UTF-8。
# 直接按 utf-8 读会把绝大部分汉字丢掉（errors="ignore" 时静默丢，
# 表现为「索引建好了但什么都搜不到」），故这里做编码探测。
_ENCODINGS = ("utf-8-sig", "gb18030", "big5hkscs", "utf-16")

# 切分参数
DEFAULT_MAX_CHARS = 600
DEFAULT_OVERLAP = 80
DEFAULT_TOP_DOCS = 3
# 同一部书最多进 top_k 几条（避免巨著独占结果）
DEFAULT_PER_DOC = 2
# 篇名拼接进片段正文的长度上限（超过这个长度更像句子而非条目标题）
MAX_HEADING_CHARS = 24


def read_text(path: Path) -> str:
    """按编码候选依次尝试解码；全部失败时退回忽略错误的 utf-8。"""
    raw = Path(path).read_bytes()
    for enc in _ENCODINGS:
        try:
            return raw.decode(enc)
        except (UnicodeDecodeError, LookupError):
            continue
    return raw.decode("utf-8", errors="ignore")


def parse_sections(text: str) -> list[tuple[str, str]]:
    """把一部典籍拆成 `(篇名, 正文)` 列表。

    典籍的通用结构：

    ```
    <篇名>伤寒论          <- 书名（元数据）
    书名：伤寒论
    作者：张仲景
    朝代：东汉
    年份：…

    <目录>
    <篇名>辨太阳病脉证并治上   <- 真正的章节
    属性：1．太阳之为病，脉浮…
    ```

    第一段只是书目元数据（书名/作者/朝代/年份），不是可检索的医学内容，
    必须剔除——否则「张仲景」「东汉」这类词会把无关查询吸到这本书上。
    """
    body = text
    toc = body.find("<目录>")
    if toc >= 0:
        body = body[toc + len("<目录>") :]
    # 去掉正文里残留的目录导航行（形如 `<目录>卷之四十一\妇人妊娠病诸候上`）
    body = _TOC_INLINE_RE.sub("", body)

    matches = list(_SECTION_RE.finditer(body))
    sections: list[tuple[str, str]] = []
    if not matches:
        cleaned = _BODY_PREFIX_RE.sub("", body).strip()
        return [("", cleaned)] if cleaned else []

    for i, m in enumerate(matches):
        start = m.end()
        end = matches[i + 1].start() if i + 1 < len(matches) else len(body)
        chunk = body[start:end]
        # 先 strip 再去标签：`^` 只锚定串首，带换行时匹配不到
        chunk = _BODY_PREFIX_RE.sub("", chunk.strip()).strip()
        if chunk:
            sections.append((m.group("name").strip(), chunk))
    return sections


def iter_chunks(text: str, max_chars: int = DEFAULT_MAX_CHARS,
                overlap: int = DEFAULT_OVERLAP) -> Iterator[tuple[str, str]]:
    """按章节切分，产出 `(篇名, 片段)`。

    短篇名（如药名「当归」、方名「桂枝汤」）会**拼进片段正文**：
    典籍里这些条目的正文常常通篇不提自己的名字（「味甘平，主咳逆上气…」），
    少了标题就既搜不到、也让模型引用时说不清出处。
    """
    for section, body in parse_sections(text):
        prefix = f"{section}：" if 0 < len(section) <= MAX_HEADING_CHARS else ""
        for c in chunk_text(body, max_chars, overlap):
            yield section, f"{prefix}{c}"


def normalize(text: str) -> str:
    """只保留汉字，用于建立索引与切分查询。"""
    return _NON_CJK_RE.sub("", text or "")


def bigrams(s: str) -> list[str]:
    """中文二元切分：'脾胃湿热' -> ['脾胃','胃湿','湿热']。"""
    if len(s) < 2:
        return [s] if s else []
    return [s[i : i + 2] for i in range(len(s) - 1)]


def count_bigrams(s: str) -> Counter:
    """统计 bigram 词频，键为**字符串**（与 `bigrams()` 保持一致）。

    计数阶段用 `zip` 走 C 层迭代（比下标循环快得多），
    最后只在**去重后的**词表上做一次拼接，避免在千万级 token 上逐个 join。
    """
    if len(s) < 2:
        return Counter([s]) if s else Counter()
    counted = Counter(zip(s, s[1:]))
    return Counter({"".join(k): v for k, v in counted.items()})


def chunk_text(text: str, max_chars: int = DEFAULT_MAX_CHARS,
               overlap: int = DEFAULT_OVERLAP) -> list[str]:
    """把长文切成检索用的片段。

    先按空行/换行分自然段，短段合并到 ``max_chars``；超长段硬切并保留
    ``overlap`` 字的重叠，避免把一句话劈成两半后两边都失去上下文。
    """
    paras = [p.strip() for p in re.split(r"[\r\n]+", text or "") if p.strip()]
    chunks: list[str] = []
    cur = ""
    for p in paras:
        while len(p) > max_chars:
            if cur:
                chunks.append(cur)
                cur = ""
            chunks.append(p[:max_chars])
            p = p[max_chars - overlap :] if max_chars > overlap else p[max_chars:]
        if not cur:
            cur = p
        elif len(cur) + len(p) <= max_chars:
            cur += p
        else:
            chunks.append(cur)
            cur = (cur[-overlap:] if overlap else "") + p
    if cur:
        chunks.append(cur)
    return [c for c in chunks if len(c) >= 8]


@dataclass
class Book:
    """一部典籍。"""

    ord: int
    doc_id: str
    title: str
    path: Path

    @property
    def kind(self) -> str:
        return "book"


@dataclass
class ChunkHit:
    """一个检索命中的片段。"""

    id: str
    score: float
    text: str
    book: str
    doc_id: str
    meta: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "score": round(self.score, 4),
            "text": self.text,
            "book": self.book,
            "doc_id": self.doc_id,
            "meta": self.meta,
        }


def iter_books(src_dir: Path) -> Iterator[Book]:
    """按文件名顺序列出语料目录下的全部 ``*.txt``。

    文件名约定 ``<序号>-<书名>.txt``；不合规的文件以文件主干名作为书名，
    保证「丢进来就能被索引」，而不是静默跳过。

    路径一律存**绝对路径**：检索时要按路径回读原文，相对路径会在换目录后
    找不到文件（表现为「索引在、但检索报 FileNotFoundError」）。
    """
    files = sorted(p for p in Path(src_dir).resolve().glob("*.txt") if p.is_file())
    for i, p in enumerate(files):
        m = _BOOK_NAME_RE.match(p.stem)
        if m:
            doc_id, title = m.group("no"), m.group("title")
        else:
            doc_id, title = p.stem, p.stem
        yield Book(ord=i, doc_id=doc_id, title=title, path=p)


class CorpusIndex:
    """书级倒排 + 片段级精排的语料索引。"""

    #: 书级排序时，df 超过「部数 × 该比例」的查询词视为泛词并忽略
    max_query_df_ratio: float = 0.25

    def __init__(self, db_path: Path) -> None:
        self.db_path = Path(db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        # check_same_thread=False：HTTP 服务里查询由线程池执行，
        # 与建索引的线程不同；并发访问由下面的锁串行化。
        self._conn = sqlite3.connect(str(self.db_path), check_same_thread=False)
        self._lock = threading.Lock()
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA synchronous=OFF")
        self._ensure_schema()

    def _query(self, sql: str, params: Iterable = ()) -> list[tuple]:
        """串行化 DB 访问：sqlite3 连接不是线程安全的。"""
        with self._lock:
            return self._conn.execute(sql, tuple(params)).fetchall()

    # ---------------- 构建 ----------------
    def _ensure_schema(self) -> None:
        c = self._conn
        c.execute(
            """CREATE TABLE IF NOT EXISTS docs(
                   ord INTEGER PRIMARY KEY,
                   doc_id TEXT,
                   title TEXT,
                   path TEXT,
                   chars INTEGER,
                   terms INTEGER)"""
        )
        c.execute(
            """CREATE TABLE IF NOT EXISTS postings(
                   term TEXT,
                   ord INTEGER,
                   tf INTEGER)"""
        )
        c.execute("CREATE TABLE IF NOT EXISTS terms(term TEXT PRIMARY KEY, df INTEGER)")
        c.execute("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT)")
        c.execute("CREATE INDEX IF NOT EXISTS idx_postings_term ON postings(term)")
        # ---- 分类标签（见 taxonomy.py）：只在 `corpus-classify` 时写入 ----
        # 单独建表而不是给 docs 加列：老索引库也能直接升级，不必重建（重建要几分钟）。
        c.execute(
            """CREATE TABLE IF NOT EXISTS doc_tags(
                   doc_id TEXT, dim TEXT, tag TEXT)"""
        )
        c.execute("CREATE INDEX IF NOT EXISTS idx_doc_tags_tag ON doc_tags(tag)")
        c.execute("CREATE INDEX IF NOT EXISTS idx_doc_tags_doc ON doc_tags(doc_id)")
        c.execute(
            """CREATE TABLE IF NOT EXISTS doc_meta(
                   doc_id TEXT PRIMARY KEY,
                   author TEXT, dynasty TEXT, era TEXT)"""
        )
        c.commit()

    def build(self, src_dir: Path, *, min_tf: int = 2,
              max_terms_per_doc: int = 3000,
              limit: int | None = None,
              progress: bool = False) -> dict:
        """重建索引（全量）。返回构建统计。"""
        c = self._conn
        c.execute("DELETE FROM docs")
        c.execute("DELETE FROM postings")
        c.execute("DELETE FROM terms")
        c.commit()

        n_books = 0
        n_chars = 0
        batch: list[tuple[str, int, int]] = []

        for book in iter_books(src_dir):
            if limit is not None and n_books >= limit:
                break
            text = normalize(read_text(book.path))
            n_chars += len(text)
            counter = count_bigrams(text)
            # 每部书只保留最具代表性的 term：既控制索引体积，
            # 也让「某书特有的词」在检索中占主导。
            kept = [(t, n) for t, n in counter.items() if n >= min_tf]
            kept.sort(key=lambda x: (-x[1], x[0]))
            kept = kept[:max_terms_per_doc]

            c.execute(
                "INSERT INTO docs(ord, doc_id, title, path, chars, terms) VALUES(?,?,?,?,?,?)",
                (book.ord, book.doc_id, book.title, str(book.path), len(text), len(kept)),
            )
            for term, n in kept:
                batch.append((term, book.ord, n))
            if len(batch) >= 200_000:
                c.executemany("INSERT INTO postings(term, ord, tf) VALUES(?,?,?)", batch)
                batch.clear()
            n_books += 1
            if progress and n_books % 50 == 0:
                print(f"  已索引 {n_books} 部…", flush=True)

        if batch:
            c.executemany("INSERT INTO postings(term, ord, tf) VALUES(?,?,?)", batch)
        c.commit()

        # df 在 SQL 侧汇总：避免把百万级 term 再在 Python 里攒一遍
        c.execute("INSERT INTO terms(term, df) SELECT term, COUNT(*) FROM postings GROUP BY term")
        c.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('books', ?), ('chars', ?), ('built_at', datetime('now'))",
            (str(n_books), str(n_chars)),
        )
        c.commit()
        c.execute("CREATE INDEX IF NOT EXISTS idx_postings_term ON postings(term)")
        c.commit()
        return {"books": n_books, "chars": n_chars, "path": str(self.db_path)}

    # ---------------- 检索 ----------------
    def stats(self) -> dict:
        books = self._query("SELECT COUNT(*) FROM docs")[0][0]
        terms = self._query("SELECT COUNT(*) FROM terms")[0][0]
        postings = self._query("SELECT COUNT(*) FROM postings")[0][0]
        chars = self._query("SELECT value FROM meta WHERE key='chars'")
        return {
            "books": int(books),
            "terms": int(terms),
            "postings": int(postings),
            "chars": int(chars[0][0]) if chars else 0,
            "db": str(self.db_path),
        }

    def _doc_lengths(self) -> tuple[int, float]:
        row = self._query("SELECT COUNT(*), COALESCE(AVG(terms), 1) FROM docs")[0]
        return int(row[0]), float(row[1])

    def _idf(self, terms: Sequence[str]) -> dict[str, float]:
        """查询词的 IDF；索引中不存在的词直接跳过（df=0 无信息量）。"""
        return self._term_stats(terms)[0]

    def _term_stats(self, terms: Sequence[str]) -> tuple[dict[str, float], dict[str, int]]:
        """返回 (idf, df)：书级排序时要用 df 过滤「满库都是」的泛词。"""
        n_docs, _ = self._doc_lengths()
        idf: dict[str, float] = {}
        df: dict[str, int] = {}
        for i in range(0, len(terms), 400):
            chunk = terms[i : i + 400]
            qs = ",".join("?" * len(chunk))
            rows = self._query(f"SELECT term, df FROM terms WHERE term IN ({qs})", chunk)
            for t, n in rows:
                df[t] = int(n)
                idf[t] = math.log(1.0 + (n_docs - n + 0.5) / (n + 0.5))
        return idf, df

    def _rank_docs(self, query_terms: Sequence[str], idf: dict[str, float],
                   dfs: dict[str, int], top_docs: int,
                   allowed: set[int] | None = None) -> list[int]:
        """书级 BM25：先找出最可能包含答案的几部书。

        `allowed` 是标签过滤后的候选书集合（`None` = 不限制）。过滤放在
        **排序之前**而不是之后：先排序再过滤会把候选全砍光（儿科书在
        BM25 里本来就排不进前三），这里是「只在某类书里找最相关的几本」。
        """
        if not query_terms:
            return []
        if allowed is not None and not allowed:
            return []
        n_docs, avg_dl = self._doc_lengths()
        # 只取最具区分度的查询词：
        # - 索引中不存在的词（无 idf）直接丢弃；
        # - 满库都是的泛词（df 超过 max_df_ratio）也丢弃——
        #   否则「妊娠禁忌候 孕妇饮食宜忌」这类复合查询会被「饮食」「宜忌」
        #   这类高频词带偏，真正罕见的「妊娠禁忌」反而排不上。
        #
        # 但**按标签过滤时不做泛词抑制**：df 是按全库算的，而候选书只剩几部。
        # 「附子」在 696 部里出现 525 部，按全库标准它是彻头彻尾的泛词；
        # 可在 5 部火神派书里，它恰恰是最该命中的词。候选集已经被标签收窄，
        # 这时再抑制只会把查询词砍光（`--tags 火神派` 查「附子干姜」返回空
        # 就是这么来的）。
        max_df = None if allowed is not None else max(1, int(n_docs * self.max_query_df_ratio))
        ranked_terms = [
            t for t in query_terms
            if t in idf and (max_df is None or dfs.get(t, n_docs + 1) <= max_df)
        ]
        # 兜底：若过滤后一个词都不剩（查询全是泛词），退回按 IDF 取最罕见的若干个
        if not ranked_terms:
            ranked_terms = [t for t in query_terms if t in idf]
        ranked_terms.sort(key=lambda t: -idf.get(t, 0.0))
        ranked_terms = ranked_terms[:32]
        if not ranked_terms:
            return []
        dl = {r[0]: r[1] for r in self._query("SELECT ord, terms FROM docs")}

        scores: dict[int, float] = {}
        k1, b = 1.5, 0.75
        for i in range(0, len(ranked_terms), 200):
            chunk = ranked_terms[i : i + 200]
            qs = ",".join("?" * len(chunk))
            rows = self._query(
                f"SELECT ord, term, tf FROM postings WHERE term IN ({qs})", chunk
            )
            for ord_, term, tf in rows:
                w = idf.get(term)
                if not w:
                    continue
                if allowed is not None and ord_ not in allowed:
                    continue
                norm = tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * (dl.get(ord_, avg_dl) / avg_dl)))
                scores[ord_] = scores.get(ord_, 0.0) + w * norm
        return [o for o, _ in sorted(scores.items(), key=lambda x: -x[1])[:top_docs]]

    def search(self, query: str, *, top_k: int = 5, top_docs: int = DEFAULT_TOP_DOCS,
               max_chars: int = DEFAULT_MAX_CHARS,
               overlap: int = DEFAULT_OVERLAP,
               per_doc: int = DEFAULT_PER_DOC,
               tags: Sequence[str] | None = None,
               tag_groups: Sequence[Sequence[str]] | None = None) -> list[ChunkHit]:
        """检索：书级 BM25 收敛 -> 片段级 IDF 覆盖打分。

        `per_doc` 限制**同一部书最多进几条**：像《普济方》《本草纲目》这种
        几百万字的巨著，命中片段数远超小书，不设上限会把 top-k 全占满，
        等于把「检索多本书」退化成「在一本书里翻页」。

        `tags` 限定检索范围（如 `["儿科"]`、`["火神派"]`，多个标签取**并集**）；
        `tag_groups` 做跨维度交集（组内并集、组间交集，语义见
        `doc_ords_for_tags`）。两者都需要先跑过 `corpus-classify` 写入
        `doc_tags` 表；未分类时传标签只会返回空结果，故这里显式报错
        而不是静默返回空。
        """
        q = normalize(query)
        q_terms = bigrams(q)
        if not q_terms:
            return []
        idf, df = self._term_stats(q_terms)
        if tags or tag_groups:
            allowed = self.doc_ords_for_tags(tags, groups=tag_groups)
        else:
            allowed = None
        # 整词（如「半夏泻心汤」）命中应显著压过零散 bigram 命中：
        # 用户问的是方名/药名，逐字拆开打分会让「恰好含 半夏 和 泻心 的段落」
        # 排在真正讲这个方的段落之前。
        phrases = [
            normalize(p) for p in _PHRASE_SPLIT_RE.split(query) if len(normalize(p)) >= 2
        ]
        doc_ords = self._rank_docs(q_terms, idf, df, top_docs, allowed=allowed)
        if not doc_ords:
            return []

        qs = ",".join("?" * len(doc_ords))
        books = self._query(
            f"SELECT ord, doc_id, title, path FROM docs WHERE ord IN ({qs})", doc_ords
        )

        hits: list[ChunkHit] = []
        for ord_, doc_id, title, path in books:
            text = read_text(Path(path))
            for i, (section, chunk) in enumerate(iter_chunks(text, max_chars, overlap)):
                cjk = normalize(chunk)
                if len(cjk) < 8:
                    continue
                # 覆盖度 × IDF：命中的查询词越少、越冷僻，得分越低；
                # 除以长度的开方，避免长片段靠堆字数取胜。
                covered = {t for t in q_terms if t in cjk}
                if not covered:
                    continue
                weight = sum(idf.get(t, 0.0) for t in covered)
                base = (len(covered) / len(q_terms)) * weight / math.sqrt(len(cjk) / max_chars)
                bonus = sum(2.0 * len(p) for p in phrases if p in cjk)
                score = base + bonus
                hits.append(
                    ChunkHit(
                        id=f"book::{doc_id}:{i}",
                        score=score,
                        text=chunk,
                        book=title,
                        doc_id=doc_id,
                        meta={
                            "doc_ord": ord_,
                            "chunk": i,
                            "section": section,
                            "chars": len(chunk),
                        },
                    )
                )
        hits.sort(key=lambda h: (-h.score, h.id))
        # 同书限流：先按分数取，再按书分组截断，最后合并重排取 top_k
        per_doc_count: dict[int, int] = {}
        diversified: list[ChunkHit] = []
        for h in hits:
            ord_ = h.meta.get("doc_ord", -1)
            n = per_doc_count.get(ord_, 0)
            if per_doc > 0 and n >= per_doc:
                continue
            per_doc_count[ord_] = n + 1
            diversified.append(h)
        return diversified[:top_k]

    # ---------------- 维护 ----------------
    def books(self, limit: int = 20) -> list[dict]:
        rows = self._query(
            "SELECT ord, doc_id, title, chars, terms FROM docs ORDER BY ord LIMIT ?", (limit,)
        )
        return [
            {"ord": r[0], "doc_id": r[1], "title": r[2], "chars": r[3], "terms": r[4]}
            for r in rows
        ]

    # ---------------- 分类标签（taxonomy.py） ----------------
    def write_classification(self, books: Iterable) -> int:
        """把分类结果写进 `doc_tags` / `doc_meta`（按 doc_id 全量覆盖）。

        接受 `taxonomy.BookMeta` 的序列。这里不 import taxonomy：
        那会造成循环引用（taxonomy 依赖本模块的 `iter_books`），
        故按鸭子类型取 `doc_id / tags / author / dynasty / era`。
        """
        c = self._conn
        with self._lock:
            c.execute("DELETE FROM doc_tags")
            c.execute("DELETE FROM doc_meta")
            tags: list[tuple[str, str, str]] = []
            metas: list[tuple[str, str, str, str]] = []
            for b in books:
                for dim, tag_list in (getattr(b, "tags", None) or {}).items():
                    for tag in tag_list:
                        tags.append((b.doc_id, dim, tag))
                metas.append((b.doc_id, getattr(b, "author", "") or "",
                              getattr(b, "dynasty", "") or "",
                              getattr(b, "era", "") or ""))
            c.executemany("INSERT INTO doc_tags(doc_id, dim, tag) VALUES(?,?,?)", tags)
            c.executemany(
                "INSERT OR REPLACE INTO doc_meta(doc_id, author, dynasty, era)"
                " VALUES(?,?,?,?)", metas)
            c.commit()
        return len(tags)

    def tag_counts(self, dim: str | None = None) -> dict[str, int]:
        """各标签的部数统计；`dim` 指定时只统计该维度。"""
        if dim:
            rows = self._query(
                "SELECT tag, COUNT(*) FROM doc_tags WHERE dim=? GROUP BY tag", (dim,))
        else:
            rows = self._query("SELECT tag, COUNT(*) FROM doc_tags GROUP BY tag")
        return {t: int(n) for t, n in sorted(rows, key=lambda r: (-r[1], r[0]))}

    def _ords_for_one_group(self, tags: Sequence[str]) -> set[int]:
        """一组标签 -> 书序号集合（组内取并集）。"""
        wanted = [t for t in (tags or ()) if t]
        if not wanted:
            return set()
        qs = ",".join("?" * len(wanted))
        rows = self._query(
            f"SELECT DISTINCT d.ord FROM docs d JOIN doc_tags t ON t.doc_id = d.doc_id"
            f" WHERE t.tag IN ({qs})", wanted)
        return {int(r[0]) for r in rows}

    def doc_ords_for_tags(self, tags: Sequence[str] | None = None, *,
                          groups: Sequence[Sequence[str]] | None = None) -> set[int]:
        """标签 -> 书序号集合。

        两种用法：

        - `tags=["儿科", "产科"]`：**并集**（任一命中即可）；
        - `groups=[["方书方剂"], ["儿科", "温病疫病"]]`：组内并集、
          **组间交集**，即「方书 AND (儿科 OR 温病)」。

        为什么需要 `groups`：四个分类维度是正交的，「儿科的方书」在
        标签层面是两个维度的交集，扁平并集会把「儿科的医案」和「内科的方书」
        一起捞进来——那不是调用方想要的。sub-agent 的检索域正是
        「体裁/功能（静态）AND 科室（动态）」这种跨维度组合。
        """
        wanted_groups = [g for g in (groups or ()) if g]
        flat = [t for t in (tags or ()) if t]
        if not wanted_groups and not flat:
            return set()

        ords: set[int] | None = None
        for g in wanted_groups:
            g_ords = self._ords_for_one_group(g)
            ords = g_ords if ords is None else (ords & g_ords)
        if flat:
            flat_ords = self._ords_for_one_group(flat)
            ords = flat_ords if ords is None else (ords | flat_ords)

        result = ords or set()
        if not result and not self._query("SELECT 1 FROM doc_tags LIMIT 1"):
            raise ValueError(
                "索引库里没有分类标签，先跑 `python -m rag corpus-classify`；"
                "否则按标签过滤只会永远返回空结果"
            )
        return result

    def close(self) -> None:
        self._conn.close()

    # Windows 上未关闭的 sqlite 连接会让临时目录删不掉，故提供上下文管理器
    def __enter__(self) -> "CorpusIndex":
        return self

    def __exit__(self, *exc) -> None:
        self.close()


def build_index(src_dir: Path, db_path: Path, **kw) -> dict:
    """便捷函数：建库并返回统计。"""
    with CorpusIndex(db_path) as idx:
        return idx.build(Path(src_dir), **kw)


def search_index(db_path: Path, query: str, **kw) -> list[dict]:
    """便捷函数：检索并返回 dict 列表。"""
    with CorpusIndex(db_path) as idx:
        return [h.to_dict() for h in idx.search(query, **kw)]


__all__ = [
    "Book", "ChunkHit", "CorpusIndex", "build_index", "search_index",
    "chunk_text", "iter_books", "iter_chunks", "parse_sections",
    "normalize", "bigrams", "read_text",
]
