/**
 * ECharts 按需注册：只挂应用实际用到的 charts/components/renderer。
 *
 * 全量 `import "echarts"` 会把地图/3D/雷达/甘特等全部图表类型拖进产物
 * （占前端包体约 2/3）。新图表类型在此追加注册，不要绕过本模块直接
 * import "echarts"——类型不够时优先怀疑漏注册（运行时报 "Unknown chart
 * series" 即为本模块缺项）。
 *
 * 图型纪律：时间序列一律柱状（直方图风格），不引入折线图（用户明确要求）。
 */
import * as echarts from "echarts/core";
import { BarChart } from "echarts/charts";
import {
  AxisPointerComponent,
  GridComponent,
  LegendComponent,
  MarkPointComponent,
  TooltipComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";

echarts.use([
  BarChart,
  GridComponent,
  TooltipComponent,
  AxisPointerComponent,
  LegendComponent,
  MarkPointComponent,
  CanvasRenderer,
]);

export { echarts };
