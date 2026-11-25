import { useState } from 'react';
import { ImageOff } from 'lucide-react';
import { Card, Hint, Tag, cn } from '@/components/ui';
import type { CatalogMedia, CatalogRelease } from '@/lib/types';
import { audienceRating, kindLabel, listOrUnknown, regionsLabel, sourceLabel, statusLabel } from './helpers';

export function Poster({ src, title, className }: { src: string | null; title: string; className?: string }) {
  const [failed, setFailed] = useState<string | null>(null);
  const usable = src && /^https?:\/\//i.test(src) && failed !== src;
  return (
    <div className={cn('aspect-[2/3] overflow-hidden rounded-md bg-sunken', className)}>
      {usable ? (
        <img src={src} alt={`${title}海报`} className="h-full w-full object-cover" loading="lazy" referrerPolicy="no-referrer" onError={() => setFailed(src)} />
      ) : (
        <div className="flex h-full flex-col items-center justify-center gap-2 p-2 text-xs text-muted" aria-label={`${title}暂无海报`}>
          <ImageOff size={24} aria-hidden />
          <span>暂无海报</span>
        </div>
      )}
    </div>
  );
}

export function ReleaseStamp({ release }: { release: CatalogRelease }) {
  return (
    <div className="space-y-1 break-words text-xs text-fg2">
      <div className="tabular-nums">{release.date} · {release.local_time || '时间未知'}{release.timezone ? `（${release.timezone}）` : '（时区未知）'}</div>
      <div>{release.season !== null ? `第 ${release.season} 季 · ` : ''}{release.episode || release.title || '暂无集/章节资料'}</div>
      <div className="text-muted">{release.release_kind === 'chapter' ? '章节发行' : release.release_kind === 'episode' ? '单集发行' : release.release_kind || '发行'} · {release.platform || '平台未知'} · {sourceLabel(release.source)}</div>
    </div>
  );
}

export function MediaCard({ media, release, onOpen }: { media: CatalogMedia; release?: CatalogRelease; onOpen: (key: string) => void }) {
  return (
    <Card hover className="flex min-w-0 cursor-pointer flex-col gap-2 p-2" role="button" tabIndex={0} aria-label={`查看 ${media.title} 元数据`} onClick={() => onOpen(media.key)} onKeyDown={(event) => {
      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onOpen(media.key); }
    }}>
      <Poster src={media.poster} title={media.title} />
      <div className="line-clamp-2 text-sm font-medium text-fg"><Hint content={media.title} focusable={false}>{media.title}</Hint></div>
      <div className="flex flex-wrap gap-1"><Tag>{kindLabel(media.kind)}</Tag><Tag>{statusLabel(media.status)}</Tag></div>
      <div className="truncate text-xs text-fg2"><Hint content={regionsLabel(media.regions)} focusable={false}>{media.year ?? '年份未知'} · {regionsLabel(media.regions)}</Hint></div>
      <div className="truncate text-xs text-muted"><Hint content={listOrUnknown(media.platforms)} focusable={false}>{listOrUnknown(media.platforms)}</Hint></div>
      <div className="text-xs text-fg2"><Hint content="单个用户/观众评分按原量表换算为十分制；不是多源综合评分" focusable={false}>{media.rating !== null ? '用户/观众 ' : ''}{audienceRating(media)}</Hint></div>
      {release && <div className="border-t border-line pt-2"><ReleaseStamp release={release} /></div>}
    </Card>
  );
}

export const CARD_GRID = 'grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4 xl:grid-cols-6 2xl:grid-cols-8';
