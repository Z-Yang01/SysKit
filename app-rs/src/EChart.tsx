/**
 * 项目统一图表组件：echarts-for-react core 版 + 按需注册的 echarts 实例
 * （见 ./echarts）。全量引入已被禁——产物里 echarts 占 2/3，按需后约减半。
 */
import ReactEChartsCore from "echarts-for-react/esm/core";
import type { EChartsReactProps } from "echarts-for-react";
import { echarts } from "./echartsSetup";

export function EChart(props: Omit<EChartsReactProps, "echarts">) {
  return <ReactEChartsCore echarts={echarts} {...props} />;
}
