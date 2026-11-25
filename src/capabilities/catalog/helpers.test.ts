import { describe, expect, it } from 'vitest';
import { KIND_ORDER, audienceRating, canRetry, formatDay, isISODate, jobStatusLabel, kindLabel, mondayISO, originalRating, provenanceText, ratingCategory, regionLabel, regionsLabel, safeMetadataUrl, shiftISO, sourceLabel, statusLabel, syncDateError, todayISO, weekdayISO, weekRange } from './helpers';

describe('来源当地日期的 ISO 周', () => {
  it.each([
    ['2026-11-23', '2026-11-23'], ['2026-11-29', '2026-11-23'],
    ['2027-01-01', '2026-12-28'], ['2024-03-03', '2024-02-26'],
  ])('把 %s 归到周一 %s', (date, monday) => expect(mondayISO(date)).toBe(monday));
  it('周查询结束日期为下一周一（不含），展示日期截至周日', () => {
    expect(weekRange('2026-11-29')).toEqual({ start: '2026-11-23', end: '2026-11-30', last: '2026-11-29' });
    expect(weekdayISO('2026-11-23')).toBe(1);
    expect(weekdayISO('2026-11-29')).toBe(7);
    expect(formatDay('2026-11-29')).toBe('2026-11-29 周日');
  });
  it('跨闰日、月末和夏令时仍按日历日偏移', () => {
    expect(shiftISO('2024-02-28', 2)).toBe('2024-03-01');
    expect(shiftISO('2026-03-08', 1)).toBe('2026-03-09');
    expect(shiftISO('2027-01-01', -1)).toBe('2026-12-31');
    expect(todayISO(new Date(2026, 10, 24, 23))).toBe('2026-11-24');
  });
  it.each(['', '2026-02-29', '2026-13-01', '2026-11-31', '2026-1-01', 'not a date'])('拒绝不合法日期 %s', (value) => {
    expect(isISODate(value)).toBe(false);
    expect(shiftISO(value, 1)).toBe('');
    expect(weekdayISO(value)).toBeNull();
  });
  it('历史回填需要有效且递增的完整区间', () => {
    expect(syncDateError('incremental', '', '')).toBeUndefined();
    expect(syncDateError('full', '', '')).toBeUndefined();
    expect(syncDateError('history', '', '')).toBeDefined();
    expect(syncDateError('history', '2026-11-24', '2026-11-24')).toContain('结束日期');
    expect(syncDateError('history', '2026-11-24', '2026-11-23')).toContain('结束日期');
    expect(syncDateError('history', '2026-11-23', '2026-11-30')).toBeUndefined();
    expect(syncDateError('full', '2026-11-23', '')).toBeDefined();
  });
});

describe('类型、发行状态与真实地区标签', () => {
  it('六种媒体仍在同一能力内', () => {
    expect(KIND_ORDER).toEqual(['variety', 'anime', 'manga', 'tv', 'movie', 'documentary']);
    expect(kindLabel('manga')).toBe('漫画');
    expect(kindLabel('future_kind')).toBe('future_kind');
    expect(statusLabel('airing')).toBe('更新中');
    expect(statusLabel('unknown')).toBe('状态未知');
    expect(jobStatusLabel('partial')).toBe('部分完成');
    expect(canRetry('failed')).toBe(true);
    expect(canRetry('partial')).toBe(true);
    expect(canRetry('success')).toBe(false);
    expect(canRetry('running')).toBe(false);
  });
  it.each([['CN', '中国大陆'], ['HK', '中国香港'], ['TW', '中国台湾'], ['JP', '日本'], ['KR', '韩国'], ['US', '美国'], ['GB', '英国'], ['TH', '泰国'], ['EU', '欧洲'], ['FR', '法国'], ['DE', '德国'], ['IT', '意大利'], ['PL', '波兰']])('保留 %s 的实际国家/地区', (code, label) => expect(regionLabel(code)).toBe(label));
  it('未知地区不冒充中国或欧洲；未知代码不丢失', () => {
    expect(regionLabel('unknown')).toBe('地区未知');
    expect(regionLabel('')).toBe('地区未知');
    expect(regionLabel('ZZ')).toBe('地区未知');
    expect(regionLabel('XX')).toBe('XX');
    expect(regionsLabel([])).toBe('地区未知');
    expect(regionsLabel(['JP', 'US'])).toBe('日本 / 美国');
  });
});

describe('评分保持原始来源与量表', () => {
  it('百分量表与十分量表分别呈现，不混成一个分数', () => {
    expect(originalRating({ value: 83, scale: 100 })).toBe('83 / 100');
    expect(originalRating({ value: 8.3, scale: 10 })).toBe('8.3 / 10');
    expect(ratingCategory('critics')).toBe('专业评论评分');
    expect(ratingCategory('audience')).toBe('观众评分');
    expect(ratingCategory('user')).toBe('用户评分');
    expect(ratingCategory('positive_rate')).toBe('好评率');
  });
  it('只有带来源的单个观众/用户十分制评分才能显示在列表', () => {
    expect(audienceRating({ rating: 83, rating_source: 'anilist' })).toBe('8.3 / 10 · AniList');
    expect(audienceRating({ rating: 80, rating_source: 'kitsu' })).toBe('8 / 10 · Kitsu');
    expect(audienceRating({ rating: 8.3, rating_source: 'tmdb' })).toBe('8.3 / 10 · TMDb');
    expect(audienceRating({ rating: 8, rating_source: null })).toBe('暂无评分');
    expect(audienceRating({ rating: null, rating_source: 'tmdb' })).toBe('暂无评分');
    expect(audienceRating({ rating: 101, rating_source: 'anilist' })).toBe('暂无评分');
    expect(audienceRating({ rating: 83, rating_source: 'new_source' })).toBe('83 · new_source（原值，量表未知）');
    expect(sourceLabel('new_source')).toBe('new_source');
  });
  it('缺失、无效量表与越界评分如实显示，零分不丢失', () => {
    expect(originalRating({ value: null, scale: 10 })).toBe('暂无评分');
    expect(originalRating({ value: 8, scale: 0 })).toBe('暂无评分');
    expect(originalRating({ value: 11, scale: 10 })).toBe('暂无评分');
    expect(originalRating({ value: NaN, scale: 10 })).toBe('暂无评分');
    expect(originalRating({ value: 0, scale: 10 })).toBe('0 / 10');
  });
});

describe('溯源仅能打开已知元数据地址', () => {
  it.each([
    ['tmdb', 'https://www.themoviedb.org/movie/123-example'],
    ['anilist', 'https://anilist.co/anime/123/Example/'],
    ['tvmaze', 'https://www.tvmaze.com/shows/123/example'],
    ['bangumi', 'https://bgm.tv/subject/123'],
    ['mangadex', 'https://mangadex.org/title/11111111-1111-1111-1111-111111111111/example'],
    ['imdb', 'https://www.imdb.com/title/tt1234567/'],
    ['rotten_tomatoes', 'https://www.rottentomatoes.com/m/example'],
    ['bilibili', 'https://www.bilibili.com/bangumi/media/md123/'],
  ])('接受 %s 已知资料路径', (source, url) => expect(safeMetadataUrl(source, url)).toBe(new URL(url).href));
  it.each([
    ['bilibili', 'https://www.bilibili.com/bangumi/play/ep123'],
    ['tencent', 'https://v.qq.com/x/cover/example/example.html'],
    ['iqiyi', 'https://www.iqiyi.com/v_example.html'],
    ['youku', 'https://v.youku.com/v_show/id_example.html'],
    ['mangadex', 'https://mangadex.org/chapter/11111111-1111-1111-1111-111111111111'],
    ['imdb', 'https://www.imdb.com/video/vi123'],
    ['tmdb', 'https://www.themoviedb.org.evil.test/movie/123'],
    ['tmdb', 'https://evil.test/movie/123'],
    ['tmdb', 'https://www.themoviedb.org/movie/123?token=secret'],
    ['tmdb', 'https://user:secret@www.themoviedb.org/movie/123'],
    ['tmdb', 'https://www.themoviedb.org:1234/movie/123'],
    ['tmdb', 'https://www.themoviedb.org/movie/123#play'],
    ['anilist', 'https://anilist.co/anime/123/%70lay/'],
    ['unknown', 'https://www.imdb.com/title/tt1234567/'],
    ['tmdb', 'javascript:alert(1)'],
  ])('拒绝 %s 资源或无法验证地址 %s', (source, url) => expect(safeMetadataUrl(source, url)).toBeNull());
  it('缺少网址时无链接；旧播放地址只保留不可点击的溯源文字', () => {
    expect(safeMetadataUrl('tmdb', null)).toBeNull();
    expect(provenanceText(null)).toBe('暂无资料');
    expect(provenanceText('https://v.youku.com/v_show/id_example.html')).toBe('https://v.youku.com/v_show/id_example.html');
    const protectedUrl = provenanceText('https://example.test/data?api_key=topsecret&token=topsecret&id=123');
    expect(protectedUrl).not.toContain('topsecret');
    expect(protectedUrl).toContain('id=123');
    expect(provenanceText('https://user:secret@example.test')).not.toContain('secret');
    expect(provenanceText('https://example.test/data#token=topsecret')).not.toContain('topsecret');
  });
});
