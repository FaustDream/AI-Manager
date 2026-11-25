import { Tag } from '@/components/ui';
import type { Column } from '@/components/ui/table';
import type { CatalogMedia, CatalogReleaseRow } from '@/lib/types';
import { ReleaseStamp } from './cards';
import { audienceRating, kindLabel, listOrUnknown, regionsLabel, statusLabel, textOrUnknown } from './helpers';

// 列表排序只交给后端：对当前分页做本地排序会制造「全库已排序」的假象。
export function catalogColumns(): Column<CatalogMedia>[] {
  return [
    { key: 'title', header: '作品', truncate: true, title: (row) => row.title, cell: (row) => <span className="font-medium text-fg">{row.title}</span> },
    { key: 'kind', header: '类型', cell: (row) => kindLabel(row.kind) },
    { key: 'summary', header: '简介', truncate: true, title: (row) => textOrUnknown(row.summary), cell: (row) => <span className="text-fg2">{textOrUnknown(row.summary)}</span> },
    { key: 'regions', header: '国家/地区', truncate: true, title: (row) => regionsLabel(row.regions), cell: (row) => regionsLabel(row.regions) },
    { key: 'genres', header: '题材', truncate: true, title: (row) => listOrUnknown(row.genres), cell: (row) => listOrUnknown(row.genres) },
    { key: 'platforms', header: '平台', truncate: true, title: (row) => listOrUnknown(row.platforms), cell: (row) => listOrUnknown(row.platforms) },
    { key: 'status', header: '发行状态', cell: (row) => <Tag>{statusLabel(row.status)}</Tag> },
    { key: 'episodes', header: '集数', cell: (row) => episodeText(row) },
    { key: 'year', header: '年份', cell: (row) => row.year ?? '暂无资料' },
    { key: 'rating', header: <span title="一个用户/观众评分按原量表换算为十分制；来源逐条标明">用户/观众评分</span>, cell: (row) => audienceRating(row) },
  ];
}

/** 集数：总集数 · 已发行；只有一边就给一边，都没有不猜。 */
function episodeText(row: Pick<CatalogMedia, 'episode_count' | 'aired_episodes'>): string {
  const { episode_count: total, aired_episodes: aired } = row;
  if (total && aired) return `${total} 集 · 已发行 ${aired}`;
  if (total) return `${total} 集`;
  if (aired) return `已发行 ${aired} 集`;
  return '暂无资料';
}

export function releaseColumns(): Column<CatalogReleaseRow>[] {
  return [
    { key: 'release', header: '确认发行 · 来源当地日期', wrap: true, cell: (row) => <ReleaseStamp release={row.release} /> },
    ...catalogColumns().filter((column) => !['genres', 'year'].includes(column.key)).map((column): Column<CatalogReleaseRow> => ({
      ...column,
      sortValue: undefined,
      title: column.title ? (row) => column.title!(row.media) : undefined,
      cell: (row) => column.cell(row.media),
    })),
  ];
}
