const { chromium } = require('playwright')

const URL = process.env.URL || 'http://localhost:10086/'

;(async () => {
  const browser = await chromium.launch()
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 }, // iPhone 12 尺寸，模拟手机浏览器
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
  })
  const page = await context.newPage()
  const errors = []
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()) })
  page.on('pageerror', e => errors.push('PAGEERROR: ' + e.message))

  await page.goto(URL, { waitUntil: 'networkidle' })
  await page.waitForSelector('.form-row', { timeout: 20000 })
  const birthTextBefore = await page.locator('.form-row', { hasText: '出生日期' })
    .locator('.form-input').innerText()
  await page.screenshot({ path: 'shot1_profile.png' })

  // 点开出生日期选择器
  await page.locator('.form-row', { hasText: '出生日期' }).click()
  await page.waitForSelector('.bdp-mask', { timeout: 8000 })
  await page.waitForTimeout(500) // 等初始 scrollTop 生效 + 动画
  await page.screenshot({ path: 'shot2_picker_open.png' })

  // 三列与选中项
  const colCount = await page.locator('.bdp-col').count()
  const selectedTexts = await page.locator('.bdp-item.selected').allInnerTexts()
  // 选中行是否含数字（无空白/乱码）
  const selectedHaveDigits = selectedTexts.map(t => /\d/.test(t))

  // 诊断：打印关键元素的 className 与计算样式
  const diag = await page.evaluate(() => {
    const out = {}
    const ids = ['bdp-mask', 'bdp-sheet', 'bdp-bd', 'bdp-col-wrap', 'bdp-col', 'bdp-item']
    ids.forEach(cls => {
      const el = document.querySelector('.' + cls)
      if (!el) { out[cls] = 'NOT_FOUND'; return }
      const cs = getComputedStyle(el)
      out[cls] = {
        tag: el.tagName,
        className: el.className,
        display: cs.display,
        height: cs.height,
        position: cs.position,
        width: cs.width,
      }
    })
    const bd = document.querySelector('.bdp-bd')
    if (bd) {
      out.bdp_bd_rect = bd.getBoundingClientRect()
      out.cols = [...document.querySelectorAll('.bdp-col-wrap')].map(w => ({
        rect: w.getBoundingClientRect(),
        className: w.className,
      }))
    }
    return out
  })
  console.log('DIAG', JSON.stringify(diag, null, 2))

  // 每列可见项文本（落在 body 矩形内的 .bdp-item）
  const visiblePerCol = await page.evaluate(() => {
    const body = document.querySelector('.bdp-bd').getBoundingClientRect()
    const wraps = [...document.querySelectorAll('.bdp-col-wrap')]
    return wraps.map((w, ci) => {
      const items = [...w.querySelectorAll('.bdp-item')]
      const vis = items.filter(it => {
        const r = it.getBoundingClientRect()
        return r.bottom > body.top && r.top < body.bottom
      }).map(it => it.innerText)
      return { ci, visibleCount: vis.length, texts: vis }
    })
  })
  console.log('VISIBLE', JSON.stringify(visiblePerCol))

  // 居中：每个选中项中心应贴近指示线中心
  const centeredOffset = await page.evaluate(() => {
    const ind = document.querySelector('.bdp-indicator').getBoundingClientRect()
    const indC = ind.top + ind.height / 2
    return [...document.querySelectorAll('.bdp-col')].map(col => {
      const sel = col.querySelector('.bdp-item.selected')
      if (!sel) return null
      const r = sel.getBoundingClientRect()
      return Math.round((r.top + r.height / 2) - indC)
    })
  })

  // bug1：打开时 body 应被锁；在年列上滚，背景 scrollY 不应变
  const bodyOverflow = await page.evaluate(() => document.body.style.overflow)
  const colBox = await page.locator('.bdp-col').first().boundingBox()
  const winBefore = await page.evaluate(() => window.scrollY)
  await page.mouse.move(colBox.x + colBox.width / 2, colBox.y + colBox.height / 2)
  await page.mouse.wheel(0, 700)
  await page.waitForTimeout(500)
  const winAfter = await page.evaluate(() => window.scrollY)
  await page.screenshot({ path: 'shot3_after_wheel.png' })

  // 选中一项并点确定，确认回写
  await page.locator('.bdp-action.ok').click()
  await page.waitForTimeout(300)
  const birthTextAfter = await page.locator('.form-row', { hasText: '出生日期' })
    .locator('.form-input').innerText()
  await page.screenshot({ path: 'shot4_after_confirm.png' })

  console.log(JSON.stringify({
    birthTextBefore,
    colCount,
    selectedTexts,
    selectedHaveDigits,
    centeredOffset,
    bodyOverflow,
    backgroundScrollBefore: winBefore,
    backgroundScrollAfter: winAfter,
    backgroundMoved: winBefore !== winAfter,
    birthTextAfter,
    errors,
  }, null, 2))

  await browser.close()
})().catch(e => { console.error('SCRIPT_ERROR', e); process.exit(1) })
