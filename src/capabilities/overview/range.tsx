// 仪表盘各区块共用的一套区间控件与查询参数。
//
// 三个区块（积分统计 / Token 统计 / 会话）各持有一份区间：积分快照、Token 采集、
// 会话索引是三份独立的账，各自的区间互不牵连。
//
// 区间一律以**起止日期**交给后端（`from` / `to`，闭区间、本地日期），不折算成天数：
// 折算天数意味着后端得自己定「最多能看多久」，而这里不设上限——用户想把区间拉到
// 有数据以来的第一天，就该给他那一天。
import { useState } from 'react';
import { Input } from '@/components/ui/form';
import { Segmented } from './metric';

/** 预设档位：今天 / 7 天 / 30 天 / 60 天（自定义另算一格）。 */
const PRESETS = [1, 7, 30, 60] as const;

/** 自定义档位的初始跨度：打开时给一个能看出形状的默认区间，不是两个空控件。 */
const CUSTOM_DEFAULT_DAYS = 30;

export type Range = {
  /** 当前档位：`'1' | '7' | '30' | '60' | 'custom'` */
  key: string;
  /** 查询用的起止日（`YYYY-MM-DD`，已保证 from <= to） */
  from: string;
  to: string;
  /** 两个日期控件的值：切回预设后仍保留，下次打开自定义不丢 */
  custom: { from: string; to: string };
  pick: (key: string) => void;
  setCustom: (patch: Partial<{ from: string; to: string }>) => void;
  /** 区间说明：KPI 标签、图表小标题、排名注释共用这一句 */
  label: string;
};

export function useRange(initialDays: number = 7): Range {
  const [key, setKey] = useState(String(initialDays));
  const [custom, setCustomState] = useState(() => ({
    from: isoOf(today(), -(CUSTOM_DEFAULT_DAYS - 1)),
    to: isoOf(today()),
  }));

  const selected = key === 'custom' ? custom : { from: isoOf(today(), -(Number(key) - 1)), to: isoOf(today()) };
  // 日期控件允许手输，倒序时按时间先后取用：后端同样会对调，这里先对齐是为了 queryKey 稳定
  const ordered = selected.from <= selected.to ? selected : { from: selected.to, to: selected.from };

  return {
    key,
    from: ordered.from,
    to: ordered.to,
    custom,
    pick: setKey,
    setCustom: (patch) => setCustomState((prev) => ({ ...prev, ...patch })),
    label:
      key === 'custom'
        ? `${ordered.from.slice(5)} ~ ${ordered.to.slice(5)}`
        : Number(key) === 1
          ? '今天'
          : `最近 ${key} 天`,
  };
}

/** 区间控件：分段档位 + 自定义档位下的起止日期。三个区块共用同一副外观。 */
export function RangeControl({ range }: { range: Range }) {
  return (
    <span className="flex flex-wrap items-center justify-end gap-1.5">
      <Segmented
        value={range.key}
        options={[
          ...PRESETS.map((days) => ({
            key: String(days),
            label: days === 1 ? '今天' : `${days} 天`,
          })),
          { key: 'custom', label: '自定义' },
        ]}
        onPick={range.pick}
      />
      {range.key === 'custom' && (
        <span className="flex items-center gap-1">
          <Input
            type="date"
            className="w-32 text-xs"
            value={range.custom.from}
            max={range.custom.to || undefined}
            onChange={(e) => range.setCustom({ from: e.target.value })}
          />
          <span>~</span>
          <Input
            type="date"
            className="w-32 text-xs"
            value={range.custom.to}
            min={range.custom.from || undefined}
            max={isoOf(today())}
            onChange={(e) => range.setCustom({ to: e.target.value })}
          />
        </span>
      )}
    </span>
  );
}

/** 本地「今天」的零点：所有日期都按本地日历算，不做 UTC 换算。 */
function today(): Date {
  const now = new Date();
  return new Date(now.getFullYear(), now.getMonth(), now.getDate());
}

/** 本地日期 → `YYYY-MM-DD`。`offset` 为相对今天的天数（负数为过去）。 */
function isoOf(day: Date, offset = 0): string {
  // 用日历日加减而不是毫秒数：跨夏令时也不会把日期算偏一天
  const date = new Date(day);
  date.setDate(date.getDate() + offset);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${p(date.getMonth() + 1)}-${p(date.getDate())}`;
}
