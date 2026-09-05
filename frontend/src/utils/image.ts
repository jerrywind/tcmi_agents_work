import Taro from '@tarojs/taro'

/**
 * 选择一张图片并转成 data URL。
 *
 * 舌象 / 手相现在走**图片采集**而非文字描述（望诊需要看真实的舌质舌苔与
 * 掌色纹理，文字说不准）。data URL 直接随 `/chat` 的 `payload.images` 发往后
 * 端，由望诊 agent 作为视觉输入喂给多模态模型——不依赖额外上传端点。
 *
 * H5 端 `chooseMedia` 给的是 blob URL，需 fetch 成 Blob 再读 base64；
 * 小程序端是本地临时文件，用 `getFileSystemManager().readFileSync` 直接读。
 */

const IS_H5 =
  (typeof process !== 'undefined' && !!(process.env && (process.env as any).TARO_ENV === 'h5')) ||
  (typeof process === 'undefined' && typeof window !== 'undefined')

function blobToDataURL(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const fr = new FileReader()
    fr.onload = () => resolve(fr.result as string)
    fr.onerror = () => reject(fr)
    fr.readAsDataURL(blob)
  })
}

export async function chooseImageAsDataURL(): Promise<string | null> {
  try {
    const res = await Taro.chooseMedia({
      count: 1,
      mediaType: ['image'],
      sourceType: ['album', 'camera'],
    })
    const file = res.tempFiles?.[0]
    if (!file || !file.tempFilePath) return null
    const path = file.tempFilePath
    if (IS_H5) {
      const blob = await (await fetch(path)).blob()
      return await blobToDataURL(blob)
    }
    const b64 = Taro.getFileSystemManager().readFileSync(path, 'base64') as string
    return `data:image/jpeg;base64,${b64}`
  } catch {
    // 用户取消选择会进到这里：返回 null，UI 保持原状即可
    return null
  }
}
