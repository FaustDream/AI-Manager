import { useEffect, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { BellRing } from 'lucide-react';
import { Button, Card, Picker, Section, SkeletonBlock, StateBlock, Tag } from '@/components/ui';
import { errMsg, queryCmd, runCmd } from '@/lib/cmd';
import { listen } from '@/lib/events';
import { notify } from '@/lib/notify';
import type { CatalogFollowRow } from '@/lib/types';
import { Poster } from './cards';
import { followLabel, kindLabel, platformSearchUrl } from './helpers';

const FOLLOW_STATES = [
  { id: 'watching', label: '在追' },
  { id: 'want', label: '想看' },
  { id: 'shelve', label: '搁置' },
  { id: 'done', label: '看完' },
] as const;

const SEEN_KEY = 'aim.ui.follow.seen';
type SeenMap = Record<string, string>;

function readSeen(): SeenMap {
  try {
    const raw = localStorage.getItem(SEEN_KEY);
    return raw ? (JSON.parse(raw) as SeenMap) : {};
  } catch {
    return {};
  }
}
function writeSeen(map: SeenMap) {
  try {
    localStorage.setItem(SEEN_KEY, JSON.stringify(map));
  } catch { /* 私密模式等场景写不进就算了 */ }
}

/** 更新提醒：catalog:updated 刷新数据后，与上次见到的「最近发行日期」比对，
 *  更大就发桌面通知 + 标「新」。首次见到的条目只记录不提醒，避免刚收藏就轰炸。 */
function useReleaseNotifications(rows: CatalogFollowRow[] | undefined) {
  const seenRef = useRef<SeenMap | null>(null);
  useEffect(() => {
    if (!rows) return;
    const seen = seenRef.current ?? readSeen();
    const next: SeenMap = { ...seen };
    const announcements: Array<[string, string]> = [];
    for (const row of rows) {
      const latest = row.latest_release;
      if (!latest) continue;
      const previous = seen[latest_release_key(row)];
      if (previous === undefined) {
        next[latest_release_key(row)] = latest;
      } else if (latest > previous) {
        next[latest_release_key(row)] = latest;
        const episode = row.latest_episode ? ` · 第 ${row.latest_episode} 集` : '';
        announcements.push([`《${row.media.title}》有更新`, `${latest}${episode}`]);
      }
    }
    seenRef.current = next;
    writeSeen(next);
    for (const [title, body] of announcements.slice(0, 5)) {
      notify(title, body, 'aim-follow');
    }
  }, [rows]);
}

function latest_release_key(row: CatalogFollowRow): string {
  return row.media.key;
}

export function CatalogFollowView({ onOpen }: { onOpen: (key: string) => void }) {
  const qc = useQueryClient();
  const [states, setStates] = useState<string[]>([]);
  const rowsQ = useQuery({
    queryKey: ['catalog', 'follow', states],
    queryFn: () => queryCmd('catalog_follow_list', { states: states.length ? states : null }),
  });
  useReleaseNotifications(rowsQ.data);

  // SSE：采集更新后让追剧列表重新取数（更新提醒的 diff 在 useReleaseNotifications）
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<unknown>('catalog:updated', () => { void qc.invalidateQueries({ queryKey: ['catalog', 'follow'] }); }).then((off) => {
      if (disposed) off(); else unlisten = off;
    });
    return () => { disposed = true; unlisten?.(); };
  }, [qc]);

  const setFollowState = async (key: string, state: string) => {
    if ((await runCmd('catalog_follow_set', { key, state })) === undefined) return;
    await qc.invalidateQueries({ queryKey: ['catalog', 'follow'] });
  };
  const unfollow = async (key: string) => {
    if ((await runCmd('catalog_follow_set', { key, state: null })) === undefined) return;
    await qc.invalidateQueries({ queryKey: ['catalog', 'follow'] });
  };
  const seen = readSeen();
  const isNew = (row: CatalogFollowRow) =>
    row.latest_release !== null && seen[latest_release_key(row)] !== row.latest_release;

  const rows = rowsQ.data ?? [];
  const statesFilter = (
    <div className="flex flex-wrap items-center gap-2">
      <span className="shrink-0 text-xs font-medium text-fg2">状态</span>
      <div className="flex flex-wrap gap-1" aria-label="追剧状态筛选">
        <Button type="button" size="sm" variant={states.length === 0 ? 'soft' : 'ghost'} aria-pressed={states.length === 0} onClick={() => setStates([])}>全部</Button>
        {FOLLOW_STATES.map((item) => (
          <Button key={item.id} type="button" size="sm" variant={states.includes(item.id) ? 'soft' : 'ghost'} aria-pressed={states.includes(item.id)}
            onClick={() => setStates((old) => (old.includes(item.id) ? old.filter((s) => s !== item.id) : [...old, item.id]))}>
            {item.label}
          </Button>
        ))}
      </div>
      <span className="ml-auto text-xs tabular-nums text-fg2">{rows.length} 部</span>
    </div>
  );

  return <div className="flex flex-col gap-3">
    <Card className="p-2.5">
      {statesFilter}
      <div className="mt-2 flex flex-wrap items-center gap-2 border-t border-line pt-2">
        <BellRing size={13} className="text-muted" aria-hidden />
        <span className="text-xs text-muted">在追的作品有新发行时弹桌面通知（需在设置里开启桌面通知）。</span>
      </div>
    </Card>
    {rowsQ.isError ? <StateBlock kind="error" title="追剧列表读取失败" detail={errMsg(rowsQ.error)} onRetry={() => void rowsQ.refetch()} />
      : rowsQ.isLoading ? <SkeletonBlock rows={6} />
        : rows.length === 0 ? <StateBlock kind="empty" title="还没有收藏的作品" detail="在媒体库打开任意作品的详情，用「追剧状态」加入追剧；有新发行时会出现在这里并弹提醒。" />
          : <div className="flex flex-col gap-2.5">
            {rows.map((row) => (
              <Card key={row.media.key} className="p-2.5">
                <div className="flex items-start gap-3">
                  <button type="button" className="shrink-0 cursor-pointer" aria-label={`打开《${row.media.title}》详情`} onClick={() => onOpen(row.media.key)}>
                    <Poster src={row.media.poster} title={row.media.title} className="w-14" />
                  </button>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <button type="button" className="break-all text-left text-sm font-medium text-fg hover:underline" onClick={() => onOpen(row.media.key)}>{row.media.title}</button>
                      <Tag>{followLabel(row.state)}</Tag>
                      {isNew(row) && <Tag>新</Tag>}
                      <span className="text-xs text-muted">{kindLabel(row.media.kind)}</span>
                    </div>
                    <div className="mt-1 text-xs text-fg2">
                      最近更新：{row.latest_release ?? '暂无发行记录'}{row.latest_episode ? ` · 第 ${row.latest_episode} 集` : ''}
                      {row.next_release ? ` · 即将更新 ${row.next_release}` : ''}
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-1.5">
                      {row.media.platforms.slice(0, 6).map((platform) => {
                        const url = platformSearchUrl(platform, row.media.title);
                        return url ? <Button key={platform} type="button" size="sm" variant="ghost" onClick={() => void runCmd('open_external', { url })}>
                          去 {platform === 'bilibili' ? '哔哩哔哩' : platform === 'iqiyi' ? '爱奇艺' : platform === 'tencent' ? '腾讯视频' : platform === 'youku' ? '优酷' : platform === 'mgtv' ? '芒果TV' : platform} 看
                        </Button> : null;
                      })}
                    </div>
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-1.5">
                    <Picker label="追剧状态" value={row.state} size="sm" onValueChange={(next) => void setFollowState(row.media.key, next)} options={FOLLOW_STATES.map((item) => ({ value: item.id, label: item.label }))} />
                    <Button type="button" size="sm" variant="ghost" onClick={() => void unfollow(row.media.key)}>取消收藏</Button>
                  </div>
                </div>
              </Card>
            ))}
          </div>}
    {rows.length > 0 && <Section title="提醒说明" bodyClassName="p-3">
      <p className="text-xs leading-5 text-muted">
        「新」徽章 = 最近发行日期比上次打开时更新。桌面通知由采集更新事件驱动（热刷新每 15 分钟一轮），
        只对状态为「在追」「想看」的作品弹提醒；点右上角 X 关闭后自动记录，不再重复弹同一条。
      </p>
    </Section>}
  </div>;
}
