"""生成一张最小合法 JPEG 作为 e2e 上传素材（1x1 灰阶）。
仅在 images/sample.jpg 不存在时生成。"""
import base64
import os

_HERE = os.path.dirname(__file__)
_DST = os.path.join(_HERE, "images", "sample.jpg")

# 1x1 灰阶 JPEG（SOI + 最小压缩数据 + EOI）
_MIN_JPEG_B64 = (
    "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRof"
    "Hh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAAB"
    "AAAAAAAAAAAAAAAAAAAAAP/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AfwD/2Q=="
)

if __name__ == "__main__":
    os.makedirs(os.path.dirname(_DST), exist_ok=True)
    if not os.path.exists(_DST):
        with open(_DST, "wb") as f:
            f.write(base64.b64decode(_MIN_JPEG_B64))
        print("created", _DST)
    else:
        print("exists", _DST)
