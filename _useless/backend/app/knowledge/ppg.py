"""PPG（光电容积脉搏波）信号解析 → 中医脉象特征。

无硬件依赖：提供 ``synthesize_ppg`` 生成贴近真实指脉氧波形的模拟信号，
``analyze_ppg`` 从采样序列提取脉率、节律、波形指标并映射为中医脉位/脉势
（浮/沉、迟/数、滑/涩、有力/无力）。硬件接入时只需把真实采样序列传给
``analyze_ppg`` 即可，无需改动上层。

仅作健康参考，不替代专业诊疗。
"""
from __future__ import annotations

import math
import random
from dataclasses import dataclass, field
from typing import Iterable


# 采样率（Hz）：指夹式脉氧仪常见 25~100Hz，这里默认 50Hz
DEFAULT_FS = 50


@dataclass
class PpgResult:
    rate_bpm: float = 0.0                 # 脉率（次/分）
    rhythm: str = "整齐"                  # 整齐 | 不齐 | 结代
    depth: str = "中"                     # 浮 | 中 | 沉
    force: str = "有力"                    # 有力 | 无力
    shape: str = "平"                     # 滑 | 涩 | 平
    amplitude: float = 0.0                # 波形幅值（归一化）
    perfusion: float = 0.0                # 灌注指数近似（波形对比度）
    signal_quality: float = 0.0           # 信号质量 0~1
    notes: str = ""


def synthesize_ppg(fs: int = DEFAULT_FS, duration_s: float = 12.0,
                  rate_bpm: float = 75.0, profile: str = "normal",
                  seed: int | None = None) -> list[float]:
    """生成模拟 PPG 采样序列。

    profile: normal | slippery(滑) | choppy(涩) | weak(无力) | taut(弦)
    返回归一化到 ~[0,1] 的采样值列表。
    """
    if seed is not None:
        random.seed(seed)
    n = int(fs * duration_s)
    beat = rate_bpm / 60.0
    out: list[float] = []
    # 基线漂移与呼吸调制
    for i in range(n):
        t = i / fs
        phase = (t * beat) % 1.0
        # 主波（收缩期快速上升）+ 重搏波（舒张期小峰）
        p = _ppg_wave(phase, profile)
        # 呼吸调制（~0.25Hz）与轻微基线漂移
        breath = 1.0 + 0.04 * math.sin(2 * math.pi * 0.25 * t)
        noise = random.uniform(-0.012, 0.012)
        out.append(max(0.0, p * breath + noise))
    return out


def _ppg_wave(phase: float, profile: str) -> float:
    """单心动周期内 PPG 形态（phase in [0,1)）。"""
    # 主波：高斯峰在 phase~0.15
    main = math.exp(-((phase - 0.15) ** 2) / (2 * 0.04))
    # 重搏波（dicrotic notch 后小峰）
    dicrotic = 0.35 * math.exp(-((phase - 0.42) ** 2) / (2 * 0.03))
    base = main + dicrotic
    if profile == "slippery":
        # 平滑饱满、重搏波更明显 → 滑脉
        base = base * 1.1
    elif profile == "choppy":
        # 波峰变钝、上升缓慢 → 涩脉
        main = math.exp(-((phase - 0.22) ** 2) / (2 * 0.06))
        base = main + 0.2 * math.exp(-((phase - 0.5) ** 2) / (2 * 0.05))
    elif profile == "weak":
        # 整体幅值低、重搏波弱 → 无力
        base = (main + 0.2 * dicrotic) * 0.6
    elif profile == "taut":
        # 弦脉：主波尖锐、重搏波弱
        base = math.exp(-((phase - 0.16) ** 2) / (2 * 0.02)) + 0.15 * dicrotic
    # 归一化到 ~0.2~1.0
    return 0.2 + 0.8 * (base / 1.45)


def analyze_ppg(samples: Iterable[float], fs: int = DEFAULT_FS) -> PpgResult:
    """从 PPG 采样序列提取脉象特征。

    采用峰值检测估算脉率与节律，并由波形幅值/灌注/形态推断脉位、脉势、脉形。
    """
    xs = [float(x) for x in samples]
    res = PpgResult()
    if len(xs) < fs * 2:
        res.notes = "采样过短，无法可靠分析"
        res.signal_quality = 0.0
        return res

    mean = sum(xs) / len(xs)
    var = sum((x - mean) ** 2 for x in xs) / len(xs)
    std = math.sqrt(var)
    if std < 1e-4:
        res.notes = "信号平坦，疑似未贴合传感器"
        res.signal_quality = 0.0
        return res

    # 去趋势（减去移动平均，窗口取 1 秒以保留单次心搏、去除呼吸基线）
    win = max(3, fs)
    detr = [xs[i] - _moving_avg(xs, i, win) for i in range(len(xs))]

    # 峰值检测：局部极大值且高于相对阈值
    thr = 0.35 * (max(detr) - min(detr))
    peaks = _detect_peaks(detr, thr, fs)
    if len(peaks) < 2:
        res.notes = "未检出清晰脉搏波"
        res.signal_quality = 0.2
        return res

    # 脉率
    intervals = [(peaks[i + 1] - peaks[i]) / fs for i in range(len(peaks) - 1)]
    mean_rr = sum(intervals) / len(intervals)
    res.rate_bpm = round(60.0 / mean_rr, 1)

    # 节律：RR 间期变异
    rr_std = math.sqrt(sum((iv - mean_rr) ** 2 for iv in intervals) / len(intervals))
    cv = rr_std / mean_rr if mean_rr else 0
    if cv > 0.25:
        res.rhythm = "结代" if rr_std > 0.4 else "不齐"
    else:
        res.rhythm = "整齐"

    # 脉位（浮/沉）：灌注指数近似 AC/DC
    amp = (max(detr) - min(detr))
    res.amplitude = round(amp, 3)
    res.perfusion = round(amp / (mean + 1e-6), 3)
    if res.perfusion > 1.8:
        res.depth = "浮"
    elif res.perfusion < 0.8:
        res.depth = "沉"
    else:
        res.depth = "中"

    # 脉力（有力/无力）：直接看波形幅值（弱脉幅值明显偏低）
    if amp < 0.5:
        res.force = "无力"
    elif amp > 0.75:
        res.force = "有力"
    else:
        res.force = "和缓"

    # 脉形（滑/涩）：上升段陡峭度（弱脉主因无力，形态判定降级为平）
    slope = _rise_slope(detr, peaks, fs)
    if res.force == "无力":
        res.shape = "平"
    elif slope > 2.6:
        res.shape = "滑"
    elif slope < 2.5:
        res.shape = "涩"
    else:
        res.shape = "平"

    # 信号质量（基于峰值规律与幅值）
    res.signal_quality = round(max(0.0, min(1.0, 1.0 - cv)), 2)
    res.notes = (f"脉率 {res.rate_bpm:.0f} 次/分，"
                 f"{res.depth}脉{res.force}、{res.shape}，节律{res.rhythm}")
    return res


def _moving_avg(xs: list[float], i: int, win: int) -> float:
    lo = max(0, i - win // 2)
    hi = min(len(xs), i + win // 2 + 1)
    seg = xs[lo:hi]
    return sum(seg) / len(seg) if seg else xs[i]


def _detect_peaks(xs: list[float], thr: float, fs: int) -> list[int]:
    peaks: list[int] = []
    min_gap = max(1, fs // 3)  # 防止同一波内重复检峰
    last = -min_gap
    for i in range(1, len(xs) - 1):
        if xs[i] >= thr and xs[i] >= xs[i - 1] and xs[i] >= xs[i + 1]:
            if i - last >= min_gap:
                peaks.append(i)
                last = i
    return peaks


def _rise_slope(detr: list[float], peaks: list[int], fs: int) -> float:
    """主波上升段平均斜率（归一化）。"""
    if not peaks:
        return 0.0
    slopes = []
    for p in peaks[: min(len(peaks), 8)]:
        # 向前的谷（前一个波峰后最低点近似为 p 前 0.5*min_gap）
        start = max(0, p - fs // 4)
        bottom = min(detr[start:p + 1]) if p > start else detr[start]
        rise = detr[p] - bottom
        span = (p - start) / fs
        if span > 0:
            slopes.append(rise / span)
    if not slopes:
        return 0.0
    return sum(slopes) / len(slopes)


# 中医脉象 → 证据 key 映射（供切诊 agent 使用）
TCM_PULSE_KEYS = ["pulse.rate", "pulse.position", "pulse.force", "pulse.shape", "pulse.rhythm"]


def to_evidences(res: PpgResult) -> list[dict]:
    """把 PpgResult 转成证据 dict 列表（source=切，高置信度）。"""
    evs = [
        {"key": "pulse.rate", "value": f"{res.rate_bpm:.0f}次/分 · 脉{'数' if res.rate_bpm >= 90 else ('迟' if res.rate_bpm <= 60 else '平')}",
         "confidence": 0.85},
        {"key": "pulse.position", "value": f"{res.depth}脉", "confidence": 0.8},
        {"key": "pulse.force", "value": res.force, "confidence": 0.8},
        {"key": "pulse.shape", "value": res.shape, "confidence": 0.75},
        {"key": "pulse.rhythm", "value": res.rhythm, "confidence": 0.8},
    ]
    return evs
