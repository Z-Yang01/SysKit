/**
 * 二维码展示/导出的纯逻辑（无 React/DOM，供 vitest 锁定）。
 *
 * SVG 由后端 qrcode crate 本地渲染（不调用第三方服务），这里只负责把它
 * 变成浏览器能消费的形态。
 */

/** SVG 文本 → 可直接给 `<img src>` / canvas 的 data URL（UTF-8 百分号编码）。 */
export function svgDataUrl(svg: string): string {
  return "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
}
