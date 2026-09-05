const { chromium } = require('playwright')
;(async () => {
  const browser = await chromium.launch()
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 3 })
  await page.goto('http://localhost:10086/', { waitUntil: 'networkidle' })
  await page.waitForTimeout(1500)
  await page.locator('.form-row', { hasText: '出生日期' }).click()
  await page.waitForSelector('.bdp-col')
  await page.waitForTimeout(400)
  const info = await page.evaluate(() => {
    const core = document.querySelectorAll('.bdp-col-wrap')[0].querySelector('.bdp-col')
    const dump = (el, d) => {
      if (!el || d > 3) return null
      const kids = [...el.children].map(k => ({
        tag: k.tagName, cls: k.className, scrollTop: k.scrollTop,
        overflowY: getComputedStyle(k).overflowY, hasShadow: !!k.shadowRoot,
        kids: dump(k, d + 1),
      }))
      return { tag: el.tagName, cls: el.className, scrollTop: el.scrollTop, overflowY: getComputedStyle(el).overflowY, hasShadow: !!el.shadowRoot, kids }
    }
    return {
      host: dump(core, 0),
      hostScrollTop: core.scrollTop,
    }
  })
  console.log('PROBE', JSON.stringify(info, null, 2))
  await browser.close()
})()
