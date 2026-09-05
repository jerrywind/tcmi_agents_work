const { chromium } = require('playwright')

;(async () => {
  const browser = await chromium.launch()
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3 })
  const errors = []
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()) })
  page.on('pageerror', e => errors.push('PAGEERR: ' + e.message))

  await page.goto('http://localhost:10086/', { waitUntil: 'networkidle' })
  await page.waitForTimeout(1500)

  // 点开出生日期选择器
  await page.locator('.form-row', { hasText: '出生日期' }).click()
  await page.waitForSelector('.bdp-col')
  await page.waitForTimeout(400)

  const readYear = () => page.evaluate(() => {
    const wraps = document.querySelectorAll('.bdp-col-wrap')
    const sel = wraps[0].querySelector('.bdp-item.selected')
    return sel ? sel.innerText : null
  })
  const before = await readYear()

  // 真实驱动年列滚动：定位内层滚动容器，禁用平滑滚动后设置 scrollTop 并派发 scroll
  const scrollYearBy = async (deltaPx) => {
    await page.evaluate((delta) => {
      const core = document.querySelectorAll('.bdp-col-wrap')[0].querySelector('.bdp-col')
      core.style.scrollBehavior = 'auto'
      core.scrollTop += delta
      core.dispatchEvent(new Event('scroll', { bubbles: true }))
    }, deltaPx)
  }

  await scrollYearBy(880)   // 向下拨约 20 年
  await page.waitForTimeout(400) // 等防抖（140ms）+ 吸附
  const afterScroll = await readYear()

  // 再拨一次，确认不会弹回原位
  await scrollYearBy(440)
  await page.waitForTimeout(400)
  const afterScroll2 = await readYear()

  await page.screenshot({ path: 'shot_s1_after_scroll.png' })

  // 点确定，确认回写
  await page.locator('.bdp-action.ok').click()
  await page.waitForTimeout(400)
  const birthAfter = await page.locator('.form-row', { hasText: '出生日期' }).locator('.form-input').innerText()
  await page.screenshot({ path: 'shot_s2_after_confirm.png' })

  console.log('SCROLL_TEST', JSON.stringify({
    before, afterScroll, afterScroll2,
    birthAfter,
    yearChanged: before !== afterScroll && afterScroll !== null,
    notReset: afterScroll !== before,   // 没有弹回初始
    errors,
  }, null, 2))

  await browser.close()
})()
