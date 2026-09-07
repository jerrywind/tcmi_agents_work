import Taro from '@tarojs/taro'
import { IS_H5 } from './platform'

/**
 * 选择一张图片并转成 data URL（舌象 / 手相的望诊用）。
 *
 * 舌象与手相走**图片采集**而非文字描述（望诊要看真实的舌质舌苔与掌色纹理，
 * 文字说不准）。data URL 随 `/chat` 的 `payload.images` 发往后端，
 * 由望诊 agent 作为视觉输入喂给多模态模型——不依赖额外上传端点。
 *
 * **必须压缩**：手机原图动辄 3–5MB，base64 后还要再膨胀约三分之一，
 * 三张叠起来就是十几 MB 的 JSON 请求体，足以让请求超时或被网关拒掉。
 * 而望诊只需要看清舌质舌苔与掌纹，长边 1280 完全够用。
 */

/** 压缩目标：长边像素上限。 */
const MAX_EDGE = 1280
/** H5 canvas 的 JPEG 质量（0–1）。 */
const JPEG_QUALITY = 0.82
/** 小程序 `compressImage` 的质量（0–100）。 */
const WEAPP_QUALITY = 80
/**
 * 硬上限：压缩后仍超过这个体积（base64 字符数）就判定不可用。
 *
 * 不静默塞进请求体——那会让整次问诊在跑了两百多秒之后才失败，
 * 用户既不知道原因，也已经等了太久。
 */
const HARD_LIMIT_CHARS = 4_000_000

/**
 * 用 `status` 字符串做判别式，而不是 `ok` 布尔值。
 *
 * 本仓库 tsconfig 的 `strict` 是 `false`，布尔判别式在该模式下**不做收窄**
 * ——`if (r.ok)` 之后的 else 分支拿不到另一支的字段，会编译不过。
 * 字符串字面量判别式在两种模式下都可靠。
 */
export type PickImageResult =
  /** 成功：`dataUrl` 可直接塞进 `payload.images` */
  | { status: 'ok'; dataUrl: string }
  /** 用户主动取消：不该打扰 */
  | { status: 'cancel' }
  /** 真的出问题了：调用方需要给出提示 */
  | { status: 'error'; message: string }

function blobToDataURL(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const fr = new FileReader()
    fr.onload = () => resolve(fr.result as string)
    fr.onerror = () => reject(fr)
    fr.readAsDataURL(blob)
  })
}

/**
 * H5：canvas 缩放。
 *
 * 拿不到 canvas、解码失败（部分机型拍的 HEIC 浏览器解不了）时**退回原图**，
 * 不硬失败——望诊少一次压缩仍能进行，而直接失败会让用户卡在这一步。
 */
function shrinkInH5(dataUrl: string): Promise<string> {
  if (typeof document === 'undefined') return Promise.resolve(dataUrl)
  return new Promise(resolve => {
    const img = new Image()
    img.onload = () => {
      try {
        const scale = Math.min(1, MAX_EDGE / Math.max(img.width, img.height))
        // 已比目标小就不放大：放大只会让体积变大，画质一点不涨
        if (scale >= 1) {
          resolve(dataUrl)
          return
        }
        const canvas = document.createElement('canvas')
        canvas.width = Math.round(img.width * scale)
        canvas.height = Math.round(img.height * scale)
        const ctx = canvas.getContext('2d')
        if (!ctx) {
          resolve(dataUrl)
          return
        }
        ctx.drawImage(img, 0, 0, canvas.width, canvas.height)
        const out = canvas.toDataURL('image/jpeg', JPEG_QUALITY)
        resolve(out.length < dataUrl.length ? out : dataUrl)
      } catch {
        resolve(dataUrl)
      }
    }
    img.onerror = () => resolve(dataUrl)
    img.src = dataUrl
  })
}

/** 小程序：用平台自带的 `compressImage`，失败则退回原图。 */
async function shrinkInWeapp(path: string, raw: string): Promise<string> {
  try {
    const c = await Taro.compressImage({ src: path, quality: WEAPP_QUALITY })
    const b64 = Taro.getFileSystemManager().readFileSync(c.tempFilePath, 'base64') as string
    const out = `data:image/jpeg;base64,${b64}`
    return out.length < raw.length ? out : raw
  } catch {
    return raw
  }
}

/**
 * 选一张图并压成 data URL。
 *
 * **保证不抛异常**：所有失败（用户取消、读取失败、图片过大）都通过返回值表达。
 * 这是刻意的——调用方不必包 try/catch，也就不会漏掉 `finally` 里解锁按钮。
 */
export async function chooseImageAsDataURL(): Promise<PickImageResult> {
  try {
    const res = await Taro.chooseMedia({
      count: 1,
      mediaType: ['image'],
      sourceType: ['album', 'camera'],
    })
    const file = res.tempFiles?.[0]
    if (!file || !file.tempFilePath) {
      return { status: 'error', message: '未取到图片' }
    }
    const path = file.tempFilePath

    let dataUrl: string
    if (IS_H5) {
      // H5 的 chooseMedia 给的是 blob URL，需 fetch 成 Blob 再读 base64
      const blob = await (await fetch(path)).blob()
      dataUrl = await blobToDataURL(blob)
      dataUrl = await shrinkInH5(dataUrl)
    } else {
      const b64 = Taro.getFileSystemManager().readFileSync(path, 'base64') as string
      dataUrl = `data:image/jpeg;base64,${b64}`
      dataUrl = await shrinkInWeapp(path, dataUrl)
    }

    if (dataUrl.length > HARD_LIMIT_CHARS) {
      return { status: 'error', message: '图片过大，请改用截图后重试' }
    }
    return { status: 'ok', dataUrl }
  } catch (e: any) {
    const msg = String(e?.errMsg || e?.message || '')
    // 用户主动取消不是错误：此前一律静默返回 null，
    // 结果「取消了」和「读取失败」在调用方看不出区别
    if (msg.includes('cancel')) return { status: 'cancel' }
    return { status: 'error', message: msg || '读取图片失败' }
  }
}
