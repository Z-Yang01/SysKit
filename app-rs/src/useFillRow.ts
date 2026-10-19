/**
 * 两栏页的「行高填满」测量：让左列表卡在行高内自己滚，而不是把整页撑出
 * 滚动条（用户实锤：WiFi 列表 32 项时标签栏+右栏跟着一起滚）。
 *
 * 原理：先找**最近的可滚动祖先**（而不是写死某个类名），行高 = 该容器的
 * `clientHeight` − 行顶相对容器内容区的位置 − 底部留白。
 *
 * **2026-10-20 修**：原先用 `document.querySelector(".ant-layout-content")`
 * 取容器，而 v1.4 已整体移除 AntD——选择器永远匹配不到，`measure()` 直接
 * return，高度一直是 undefined，于是整页（含标签栏）又被条目数撑开，
 * 正是这次用户报的「滚动 WiFi 时整页在滚」。改为按计算样式向上找滚动祖先，
 * 以后再换布局也不会失效。
 */
import { useEffect, useRef, useState } from "react";

/** 最近的纵向滚动祖先（overflow-y: auto|scroll）；找不到返回 null。 */
function nearestScrollParent(el: HTMLElement | null): HTMLElement | null {
  let p = el?.parentElement ?? null;
  while (p) {
    const oy = window.getComputedStyle(p).overflowY;
    if (oy === "auto" || oy === "scroll" || oy === "overlay") return p;
    p = p.parentElement;
  }
  return null;
}

/** 容器可见高与行在其内容区中的偏移 → 行高（下限 220，兜底极矮窗口）。 */
export function fillRowHeight(containerClientH: number, rowOffset: number, bottomPad = 14): number {
  return Math.max(220, Math.floor(containerClientH - rowOffset - bottomPad));
}

/**
 * 返回挂到行 div 上的 ref 与测量出的高度（首帧 undefined，随后一拍到位）。
 * active=false（如密码库锁定态）时不测量，行保持自然高度。
 */
export function useFillRowHeight(active = true) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [h, setH] = useState<number>();
  useEffect(() => {
    if (!active) return;
    const measure = () => {
      const el = ref.current;
      if (!el || el.offsetParent === null) return; // 未激活的 Tabs pane
      const c = nearestScrollParent(el);
      if (!c) return;
      const cRect = c.getBoundingClientRect();
      // 相对容器**内容区顶部**：减去容器自身 padding，再加上已滚动的距离
      // （否则容器被滚过以后 rowTop 会变成负数，算出来的高度偏大）
      const rowTop = el.getBoundingClientRect().top - cRect.top + c.scrollTop;
      const next = fillRowHeight(c.clientHeight, rowTop);
      setH((prev) => (prev === next ? prev : next));
    };
    measure();
    const t = window.setTimeout(measure, 80); // Tabs 切换动画/布局稳定后补一拍
    window.addEventListener("resize", measure);
    // 容器尺寸变化也要重测（侧栏折叠、窗口最大化等不一定触发 window.resize）
    const ro = new ResizeObserver(measure);
    if (ref.current) ro.observe(ref.current);
    const c = ref.current ? nearestScrollParent(ref.current) : null;
    if (c) ro.observe(c);
    return () => {
      window.clearTimeout(t);
      window.removeEventListener("resize", measure);
      ro.disconnect();
    };
  }, [active]);
  return { ref, h };
}
