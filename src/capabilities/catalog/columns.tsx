import { Tag } from '@/components/ui';
import type { Column } from '@/components/ui/table';
import type { CatalogMedia, CatalogReleaseRow } from '@/lib/types';
import { ReleaseStamp } from './cards';
import { audienceRating, kindLabel, listOrUnknown, regionsLabel, statusLabel } from './helpers';

// 列表排序只交给后端：对当前分页做本地排序会制造「全库已排序」的假象。
export function catalogColumns(): Column<CatalogMedia>[] {
  return [
    { key: 'title', header: '作品', truncate: true, title: (row) => row.title, cell: (row) => <span className="font-medium text-fg">{row.title}</span> },
    { key: 'kind', header: '类型', cell: (row) => kindLabel(row.kind) },
    { key: 'regions', header: '国家/地区', truncate: true, title: (row) => regionsLabel(row.regions), cell: (row) => regionsLabel(row.regions) },
    { key: 'genres', header: '题材', truncate: true, title: (row) => listOrUnknown(row.genres), cell: (row) => listOrUnknown(row.genres) },
    { key: 'platforms', header: '平台', truncate: true, title: (row) => listOrUnknown(row.platforms), cell: (row) => listOrUnknown(row.platforms) },
    { key: 'status', header: '发行状态', cell: (row) => <Tag>{statusLabel(row.status)}</Tag> },
    { key: 'year', header: '年份', cell: (row) => row.year ?? '暂无资料' },
    { key: 'rating', header: <span title="一个用户/观众评分按原量表换算为十分制；来源逐条标明">用户/观众评分</span>, cell: (row) => audienceRating(row) },
  ];
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
