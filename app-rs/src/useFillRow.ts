/**
 * 两栏页的「行高填满」测量：让左列表卡在行高内自己滚，而不是把整页撑出
 * 滚动条（用户实锤：WiFi 列表 32 项时标签栏+右栏跟着一起滚）。
 *
 * 原理：行顶 = 行相对视口的位置；行高 = Content 视口高 - 行顶 - Content
 * 底部 padding。每次 resize / 行尺寸变化重测；面板隐藏（offsetParent 为
 * null，如未激活的 Tabs pane）时跳过，高度保持上次值——同布局下再次
 * 显示仍然正确。
 */
import { useEffect, useRef, useState } from "react";

/** Content 视口高与行顶 → 行高（下限 220，兜底极矮窗口）。 */
export function fillRowHeight(contentViewH: number, rowTop: number, bottomPad = 14): number {
  return Math.max(220, Math.floor(contentViewH - rowTop - bottomPad));
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
      const c = document.querySelector<HTMLElement>(".ant-layout-content");
      if (!el || !c || el.offsetParent === null) return;
      const next = fillRowHeight(c.clientHeight, el.getBoundingClientRect().top);
      setH((prev) => (prev === next ? prev : next));
    };
    measure();
    const t = window.setTimeout(measure, 80); // Tabs 切换动画/布局稳定后补一拍
    window.addEventListener("resize", measure);
    const ro = new ResizeObserver(measure);
    if (ref.current) ro.observe(ref.current);
    return () => {
      window.clearTimeout(t);
      window.removeEventListener("resize", measure);
      ro.disconnect();
    };
  }, [active]);
  return { ref, h };
}
