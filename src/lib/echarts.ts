// ECharts 按需注册：只装仪表盘用得到的三种图与六个组件。
//
// 全量 `import 'echarts'` 会把地图、3D、桑基图等几十个用不上的部分拉进依赖图，
// 体积直接翻倍；走 echarts/core 按需注册后，产物里只有这里列出的这一份。
// 因此仓库里所有图表组件一律 `import ... from '@/lib/echarts'`，
// 不要直接 import 'echarts'——那会让按需注册白做。
import * as echarts from 'echarts/core';
import { BarChart, LineChart, PieChart } from 'echarts/charts';
import {
  DataZoomComponent,
  GridComponent,
  LegendComponent,
  MarkPointComponent,
  TooltipComponent,
} from 'echarts/components';
import { CanvasRenderer } from 'echarts/renderers';

echarts.use([
  // 图：双向柱（收支）、折线（趋势/用量）、环形（占比）
  BarChart,
  LineChart,
  PieChart,
  // 组件：直角坐标系、提示框、图例（点选隐藏系列）、缩放拖拽、最高/最低标记
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
  MarkPointComponent,
  // 渲染器：canvas 一份就够（SVG 渲染器不注册）
  CanvasRenderer,
]);

export { echarts };

import type { BarSeriesOption, LineSeriesOption, PieSeriesOption } from 'echarts/charts';
import type {
  DataZoomComponentOption,
  GridComponentOption,
  LegendComponentOption,
  MarkPointComponentOption,
  TooltipComponentOption,
} from 'echarts/components';

/** 本仓库按需注册后的合法 option：系列与组件逐项组合，写错字段编译期就能发现。 */
export type ChartOption = echarts.ComposeOption<
  | BarSeriesOption
  | LineSeriesOption
  | PieSeriesOption
  | GridComponentOption
  | TooltipComponentOption
  | LegendComponentOption
  | DataZoomComponentOption
  | MarkPointComponentOption
>;

export type { EChartsType } from 'echarts/core';
// 回调参数类型只从根包的类型面导出（echarts 6 的公开别名）；import type 编译期擦除，
// 不会把全量 echarts 拉进产物
export type {
  DefaultLabelFormatterCallbackParams as CallbackDataParams,
  TooltipComponentFormatterCallbackParams as TopLevelFormatterParams,
} from 'echarts';
export type { BarSeriesOption, LineSeriesOption, PieSeriesOption } from 'echarts/charts';
export type {
  DataZoomComponentOption,
  GridComponentOption,
  LegendComponentOption,
  MarkPointComponentOption,
  TooltipComponentOption,
} from 'echarts/components';
