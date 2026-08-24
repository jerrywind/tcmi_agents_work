/// <reference types="@tarojs/taro" />

declare module '*.scss'
declare module '*.png'
declare module '*.jpg'

declare namespace NodeJS {
  interface ProcessEnv {
    TARO_ENV: 'weapp' | 'h5' | 'rn' | 'swan' | 'alipay' | 'tt' | 'qq' | 'jd'
  }
}
