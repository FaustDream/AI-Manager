import type { ReactNode } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { ExternalLink } from 'lucide-react';
import { Button, Drawer, Picker, Section, SkeletonBlock, StateBlock, Tag } from '@/components/ui';
import { DataTable, type Column } from '@/components/ui/table';
import { errMsg, queryCmd, runCmd } from '@/lib/cmd';
import type { CatalogRating, CatalogRelease, CatalogSeason, CatalogSourceInfo, CatalogDupeCandidate, CatalogFollowRow } from '@/lib/types';
import { Poster, ReleaseStamp } from './cards';
import { followLabel, kindLabel, listOrUnknown, originalRating, platformSearchUrl, provenanceText, ratingCategory, regionsLabel, safeMetadataUrl, sourceLabel, statusLabel, textOrUnknown } from './helpers';

export function MetadataLink({ source, url }: { source: string; url: string | null }) {
  const verified = safeMetadataUrl(source, url);
  return (
    <div className="min-w-0 space-y-1">
      <div className="break-all text-xs text-muted">{provenanceText(url)}</div>
      {verified && <Button type="button" size="sm" variant="ghost" aria-label={`打开 ${sourceLabel(source)} 资料页`} onClick={() => void runCmd('open_external', { url: verified })}><ExternalLink size={12} aria-hidden />资料来源</Button>}
    </div>
  );
}

const ratingColumns: Column<CatalogRating>[] = [
  { key: 'source', header: '评分来源 / 类别', wrap: true, cell: (row) => <div>{sourceLabel(row.source)}<div className="text-xs text-muted">{ratingCategory(row.category)}</div></div> },
  { key: 'value', header: '原始评分 / 满分', cell: originalRating },
  { key: 'positive', header: '好评率', cell: (row) => row.positive_percent !== null && Number.isFinite(row.positive_percent) && row.positive_percent >= 0 && row.positive_percent <= 100 ? `${row.positive_percent}%` : '暂无资料' },
  { key: 'votes', header: '投票 / 评论数', wrap: true, cell: (row) => <div>投票 {row.votes ?? '暂无资料'}<div>评论 {row.reviews ?? '暂无资料'}</div></div> },
  { key: 'rank', header: '排名', cell: (row) => row.rank ?? '暂无资料' },
  { key: 'provenance', header: '来源网址 / 更新', wrap: true, cell: (row) => <div><MetadataLink source={row.source} url={row.url} /><div className="mt-1 text-xs text-muted">{textOrUnknown(row.updated_at)}</div></div> },
];
const seasonColumns: Column<CatalogSeason>[] = [
  { key: 'season', header: '季度', cell: (row) => row.number === null ? '暂无资料' : `第 ${row.number} 季` },
  { key: 'title', header: '标题', wrap: true, cell: (row) => textOrUnknown(row.title) },
  { key: 'episodes', header: '集数', cell: (row) => row.episode_count ?? '暂无资料' },
  { key: 'dates', header: '首播 / 完结日期', wrap: true, cell: (row) => <div>{textOrUnknown(row.premiered)}<div>{textOrUnknown(row.ended)}</div></div> },
  { key: 'source', header: '来源 / 外部 ID', wrap: true, cell: (row) => <div>{sourceLabel(row.source)}<div className="text-xs text-muted">{textOrUnknown(row.external_id)}</div></div> },
];
const releaseDetailColumns: Column<CatalogRelease>[] = [
  { key: 'release', header: '确认发行', wrap: true, cell: (row) => <ReleaseStamp release={row} /> },
  { key: 'utc', header: 'UTC 时间', cell: (row) => textOrUnknown(row.utc_time) },
  { key: 'source', header: '外部 ID / 更新 / 来源', wrap: true, cell: (row) => <div><div>{textOrUnknown(row.external_id)}</div><div className="text-xs text-muted">{textOrUnknown(row.updated_at)}</div><MetadataLink source={row.source} url={row.url} /></div> },
];
const sourceInfoColumns: Column<CatalogSourceInfo>[] = [
  { key: 'source', header: '采集来源', cell: (row) => sourceLabel(row.source) },
  { key: 'title', header: '来源标题', wrap: true, cell: (row) => textOrUnknown(row.title) },
  { key: 'episodes', header: '集数信息', wrap: true, cell: (row) => textOrUnknown(row.episode_text) },
  { key: 'url', header: '资料页 / 更新', wrap: true, cell: (row) => <div><MetadataLink source={row.source} url={row.url} /><div className="mt-1 text-xs text-muted">{textOrUnknown(row.updated_at)}</div></div> },
];

function Info({ label, children }: { label: string; children: ReactNode }) {
  return <div className="min-w-0"><dt className="text-xs text-muted">{label}</dt><dd className="mt-1 break-words text-sm text-fg2">{children}</dd></div>;
}

export function CatalogDetailDrawer({ mediaKey, onClose }: { mediaKey: string | null; onClose: () => void }) {
  const qc = useQueryClient();
  const query = useQuery({
    queryKey: ['catalog', 'detail', mediaKey],
    queryFn: () => queryCmd('catalog_detail', { key: mediaKey! }),
    enabled: mediaKey !== null,
  });
  const detail = query.data;
  const media = detail?.media;
  const followQ = useQuery({
    queryKey: ['catalog', 'follow-row', mediaKey],
    queryFn: async () => {
      const rows = await queryCmd('catalog_follow_list', { states: null });
      return rows.find((row) => row.media.key === mediaKey) ?? null;
    },
    enabled: mediaKey !== null,
  });
  const follow: CatalogFollowRow | null = followQ.data ?? null;

  const setFollow = async (state: string | null) => {
    if (mediaKey === null) return;
    if ((await runCmd('catalog_follow_set', { key: mediaKey, state })) === undefined) return;
    await qc.invalidateQueries({ queryKey: ['catalog', 'follow'] });
    await followQ.refetch();
  };

  const handleMerge = async (candidate: CatalogDupeCandidate, merge: boolean) => {
    if (!media) return;
    // 保留当前正在查看的条目，把另一条并入（或标记「不是同一作品」）
    const drop = candidate.key_a === media.key ? candidate.key_b : candidate.key_a;
    const message = await runCmd('catalog_merge', { request: { keep: media.key, drop, merge } });
    if (message !== undefined) {
      void qc.invalidateQueries({ queryKey: ['catalog'] });
      if (merge) onClose();
    }
  };

  return (
    <Drawer open={mediaKey !== null} title={media?.title || '作品资料'} subtitle={media ? `${kindLabel(media.kind)} · ${statusLabel(media.status)}` : undefined} onClose={onClose} width={800}>
      {query.isError ? <StateBlock kind="error" title="资料读取失败" detail={errMsg(query.error)} onRetry={() => void query.refetch()} /> : query.isLoading ? <SkeletonBlock rows={12} /> : !detail || !media ? <StateBlock kind="empty" title="暂无作品资料" detail="条目可能尚未采集完整或已被合并；请返回媒体库重新查询。" /> : (
        <div className="flex flex-col gap-4">
          <div className="flex items-start gap-4">
            <Poster src={media.poster} title={media.title} className="w-32 shrink-0" />
            <dl className="grid min-w-0 flex-1 grid-cols-1 gap-3 sm:grid-cols-2">
              <Info label="标题">{textOrUnknown(media.title)}</Info>
              <Info label="原名">{textOrUnknown(media.original_title)}</Info>
              <Info label="英文名">{textOrUnknown(media.english_title)}</Info>
              <Info label="别名">{listOrUnknown(media.aliases)}</Info>
              <Info label="类型 / 子类型">{kindLabel(media.kind)} / {textOrUnknown(media.subkind)}</Info>
              <Info label="发行状态"><Tag>{statusLabel(media.status)}</Tag></Info>
            </dl>
          </div>
          <Section title="追剧状态" bodyClassName="p-3">
            <div className="flex flex-wrap items-center gap-2">
              {follow ? <>
                <Tag>{followLabel(follow.state)}</Tag>
                <Picker label="追剧状态" value={follow.state} size="sm" onValueChange={(next) => void setFollow(next)} options={[
                  { value: 'watching', label: '在追' },
                  { value: 'want', label: '想看' },
                  { value: 'shelve', label: '搁置' },
                  { value: 'done', label: '看完' },
                ]} />
                <Button type="button" size="sm" variant="ghost" onClick={() => void setFollow(null)}>取消收藏</Button>
                <span className="text-xs text-muted">在追的作品有新发行时，会在「我的追剧」标记并弹桌面通知</span>
              </> : <>
                <Button type="button" size="sm" variant="soft" onClick={() => void setFollow('watching')}>＋ 加入追剧</Button>
                <Picker label="追剧状态" value="" size="sm" onValueChange={(next) => void setFollow(next)} options={[
                  { value: '', label: '直接标记为…' },
                  { value: 'want', label: '想看' },
                  { value: 'shelve', label: '搁置' },
                  { value: 'done', label: '看完' },
                ]} />
                <span className="text-xs text-muted">收藏后可在「我的追剧」跟踪更新并接收提醒</span>
              </>}
            </div>
          </Section>
          <Section title="简介" bodyClassName="p-3"><p className="whitespace-pre-wrap text-sm leading-6 text-fg2">{textOrUnknown(media.summary)}</p></Section>
          <Section title="作品元数据" bodyClassName="p-3">
            <dl className="grid grid-cols-2 gap-3">
              <Info label="制作国家 / 地区">{media.regions.length ? regionsLabel(media.regions) : '暂无资料（地区未知）'}</Info>
              <Info label="题材">{listOrUnknown(media.genres)}</Info>
              <Info label="导演">{listOrUnknown(media.directors)}</Info>
              <Info label="编剧 / 作者">{listOrUnknown(media.writers)}</Info>
              <Info label="演员 / 声优">{listOrUnknown(media.cast)}</Info>
              <Info label="制作公司">{listOrUnknown(media.studios)}</Info>
              <Info label="发行商">{listOrUnknown(media.distributors)}</Info>
              <Info label="平台">{listOrUnknown(media.platforms)}</Info>
              <Info label="年份">{media.year ?? '暂无资料'}</Info>
              <Info label="首播 / 发行日期">{textOrUnknown(media.premiered)}</Info>
              <Info label="完结日期">{textOrUnknown(media.ended)}</Info>
              {media.kind === 'manga' ? <>
                <Info label="章节数">{media.chapter_count ?? '暂无资料'}</Info>
                <Info label="最新章节">{textOrUnknown(media.latest_chapter)}</Info>
              </> : <>
                <Info label="季数">{media.season_count ?? '暂无资料'}</Info>
                <Info label="总集数">{media.episode_count ?? '暂无资料'}</Info>
                <Info label="已发行集数">{media.aired_episodes ?? '暂无资料'}</Info>
                <Info label="时长">{media.runtime === null ? '暂无资料' : `${media.runtime} 分钟`}</Info>
              </>}
            </dl>
          </Section>
          <Section title="来源评分" flush>
            {detail.ratings.length ? <div className="overflow-x-auto"><DataTable columns={ratingColumns} rows={detail.ratings} rowKey={(row) => `${row.source}:${row.category}`} dense /></div> : <StateBlock kind="empty" title="暂无评分" detail="尚未采集到授权来源的原始评分；不会用其他来源补造评分。" />}
          </Section>
          <Section title="各来源信息" flush>
            {detail.source_infos.length ? (
              <div className="overflow-x-auto">
                <p className="px-3 pt-2 text-xs text-muted">同一作品在不同采集平台各有一条落地信息；跨源合并后各来源的标题、网址与集数口径逐条保留，不会互相覆盖。</p>
                <div className="p-2"><DataTable columns={sourceInfoColumns} rows={detail.source_infos} rowKey={(row) => row.source} dense /></div>
              </div>
            ) : <StateBlock kind="empty" title="暂无来源信息" detail="该条目来自历史迁移，尚未重新采集；下次采集后会出现各来源的落地信息。" />}
          </Section>
          {media.platforms.length > 0 && <Section title="观看入口" flush>
            <div className="p-2">
              <p className="px-1 pb-2 text-xs text-muted">按已采集的来源平台聚合的官方页面入口；没有详情直链的平台给站内搜索页。只做跳转，不内嵌播放。</p>
              <div className="flex flex-wrap gap-1.5 px-1">
                {(() => {
                  const entries: Array<{ platform: string; url: string; direct: boolean }> = [];
                  const seen = new Set<string>();
                  for (const info of detail.source_infos) {
                    if (!info.url) continue;
                    entries.push({ platform: info.source, url: info.url, direct: true });
                    seen.add(info.source);
                  }
                  for (const platform of media.platforms) {
                    if (seen.has(platform)) continue;
                    const url = platformSearchUrl(platform, media.title);
                    if (url) entries.push({ platform, url, direct: false });
                  }
                  return entries.map((entry) => (
                    <Button key={`${entry.platform}:${entry.url}`} type="button" size="sm" variant={entry.direct ? 'soft' : 'ghost'}
                      onClick={() => void runCmd('open_external', { url: entry.url })}>
                      {sourceLabel(entry.platform)}{entry.direct ? '' : ' · 搜索'}
                    </Button>
                  ));
                })()}
              </div>
            </div>
          </Section>}
          {detail.dupe_candidates.length > 0 && <Section title="疑似重复作品" flush>
            <div className="p-2">
              <p className="px-1 pb-2 text-xs text-muted">标题一致但年份缺失或冲突，证据不足以自动合并——请人工确认。确认是同一作品就并进当前条目（各来源信息保留）；不是同一作品则忽略提示。</p>
              {detail.dupe_candidates.map((candidate) => {
                const other = candidate.key_a === media.key ? candidate.key_b : candidate.key_a;
                const otherTitle = candidate.key_a === media.key ? candidate.title_b : candidate.title_a;
                const otherYear = candidate.key_a === media.key ? candidate.year_b : candidate.year_a;
                return <div key={`${candidate.key_a}:${candidate.key_b}`} className="flex flex-wrap items-center gap-2 border-t border-line px-1 py-2 text-sm text-fg2">
                  <span className="min-w-0 flex-1 break-all">{otherTitle || other}<span className="text-xs text-muted"> · {other ?? ''}{otherYear ? ` · ${otherYear} 年` : ''}</span></span>
                  <Button type="button" size="sm" variant="soft" onClick={() => void handleMerge(candidate, true)}>并入当前</Button>
                  <Button type="button" size="sm" variant="ghost" onClick={() => void handleMerge(candidate, false)}>不是同一作品</Button>
                </div>;
              })}
            </div>
          </Section>}
          {detail.seasons.length > 0 && <Section title="季度资料" flush><div className="overflow-x-auto"><DataTable columns={seasonColumns} rows={detail.seasons} rowKey={(row) => `${row.source}:${row.external_id}`} dense /></div></Section>}
          <Section title="资料溯源" bodyClassName="p-3">
            <dl className="grid grid-cols-2 gap-3">
              <Info label="主资料来源">{sourceLabel(media.source)}</Info>
              <Info label="资料库 ID">{media.key}</Info>
              <Info label="首次收录">{textOrUnknown(media.first_seen)}</Info>
              <Info label="最后更新">{textOrUnknown(media.updated_at)}</Info>
            </dl>
            <div className="mt-3"><MetadataLink source={media.source} url={media.source_url} /></div>
            <div className="mt-3 space-y-1 text-sm text-fg2">
              <div className="text-xs text-muted">外部 ID</div>
              {detail.external_ids.length ? detail.external_ids.map((row) => <div key={`${row.source}:${row.external_id}`} className="break-all">{sourceLabel(row.source)} · {textOrUnknown(row.external_id)}</div>) : '暂无资料'}
            </div>
          </Section>
          <Section title="已采集的确认发行" flush>
            {detail.releases.length ? <div className="overflow-x-auto"><DataTable columns={releaseDetailColumns} rows={detail.releases} rowKey={(row) => row.key} dense /></div> : <StateBlock kind="empty" title="未采集到已确认更新，不代表当天无作品" detail="来源可能只有作品元数据，没有可确认的单集或章节发行日期。" />}
          </Section>
        </div>
      )}
    </Drawer>
  );
}
