/**
 * 端能力判断（H5 / 微信小程序 / Node 单测）。
 *
 * 全项目**只此一份**：此前 `services/harness.ts` 与 `utils/image.ts` 各写了一套
 * `IS_H5`，判定条件还略有出入。改一处漏一处就会出现「同一份代码在两个
 * 地方走了不同分支」，而这类分支错误不会报错，只会让某一端悄悄走错路。
 *
 * 判据优先用构建期常量 `process.env.TARO_ENV`（webpack 会把它字面替换成
 * `'h5'` / `'weapp'`），未被替换时（浏览器里**没有** `process`）退化为环境推断。
 */

/**
 * 读取构建期环境变量。
 *
 * **浏览器里没有 `process`**。模块顶层直接写 `process.env.X` 会抛
 * ReferenceError，整个模块加载失败 → 页面白屏，只剩一个导航栏。
 * H5 端曾因此完全不可用，而编译能过、单测也全绿——
 * 只有真机打开页面才会暴露。
 */
export function envVar(key: string): string | undefined {
  if (typeof process === 'undefined' || !process.env) return undefined
  return (process.env as Record<string, string | undefined>)[key]
}

/** 构建期目标端：`h5` / `weapp`；未替换时为空串（如 Node 单测）。 */
export const TARO_ENV = envVar('TARO_ENV') ?? ''

/** 是否为微信小程序端。 */
export const IS_WEAPP = TARO_ENV === 'weapp'

/**
 * 是否为 H5 端。
 *
 * 三种环境都能区分开：
 * - H5 浏览器：无 `process`、有 `window` → true
 * - 小程序：两者都没有 → false
 * - Node（单测）：有 `process` 且 `TARO_ENV` 未设 → false，与既有测试预期一致
 *
 * 不用 `Taro.getEnv()`：它在单测环境里并不存在（会抛 `getEnv is not a function`）。
 */
export const IS_H5 =
  TARO_ENV === 'h5' ||
  (typeof process === 'undefined' && typeof window !== 'undefined')
