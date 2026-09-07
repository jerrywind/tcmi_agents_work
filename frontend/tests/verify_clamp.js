const { chromium } = require('playwright')
;(async () => {
  const browser = await chromium.launch()
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3 })
  const errors = []
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()) })
  page.on('pageerror', e => errors.push('PAGEERR: ' + e.message))
  await page.goto('http://localhost:10086/', { waitUntil: 'networkidle' })
  await page.waitForTimeout(1500)
  await page.locator('.form-row', { hasText: '出生日期' }).click()
  await page.waitForSelector('.bdp-col')
  await page.waitForTimeout(400)

  const readCol = (ci) => page.evaluate((c) => {
    const wraps = document.querySelectorAll('.bdp-col-wrap')
    const sel = wraps[c].querySelector('.bdp-item.selected')
    return sel ? sel.innerText : null
  }, ci)

  const scrollCol = async (ci, deltaPx) => {
    await page.evaluate(({ c, d }) => {
      const core = document.querySelectorAll('.bdp-col-wrap')[c].querySelector('.bdp-col')
      core.style.scrollBehavior = 'auto'
      core.scrollTop += d
      core.dispatchEvent(new Event('scroll', { bubbles: true }))
    }, { c: ci, d: deltaPx })
    await page.waitForTimeout(300)
  }

  const ITEM_H = 44
  // 默认 1996-09-04：年=1996, 月=9(索引8), 日=4(索引3)
  const y0 = await readCol(0), m0 = await readCol(1), d0 = await readCol(2)
  // 日拨到 31 日（索引30）：delta=(30-3)*44
  await scrollCol(2, (30 - 3) * ITEM_H)
  const d31 = await readCol(2)
  // 月拨到 2 月（索引1）：delta=(1-8)*44
  await scrollCol(1, (1 - 8) * ITEM_H)
  const mFeb = await readCol(1), dAfterFeb = await readCol(2)
  await page.screenshot({ path: 'shot_c1.png' })
  await page.locator('.bdp-action.ok').click()
  await page.waitForTimeout(400)
  const birthAfter = await page.locator('.form-row', { hasText: '出生日期' }).locator('.form-input').innerText()

  console.log('CLAMP_TEST', JSON.stringify({
    initial: [y0, m0, d0],
    dayTo31: d31,
    monthToFeb: mFeb,
    dayClampedAfterFeb: dAfterFeb,
    expectedDayClamp: '28日',
    birthAfter,
    clampOk: dAfterFeb === '28日' && birthAfter === '1996-02-28',
    errors,
  }, null, 2))
  await browser.close()
})()
