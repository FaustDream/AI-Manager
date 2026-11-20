// C1 的错误话术：errMsg（kind→下一步指引）与 errText（取 message）都是纯函数。
import { describe, expect, it, vi } from 'vitest';
import { errKind, errText } from './ipc';
import { argsSummary, errMsg, queryCmd, resultSummary, runCmd } from './cmd';

const appError = (kind: string, message: string) => JSON.stringify({ kind, message });

describe('errText', () => {
  it('JSON 字符串里的 AppError 取 message', () => {
    expect(errText(appError('conflict', '「x」已存在'))).toBe('「x」已存在');
  });

  it('普通字符串原样返回，Error 取 message', () => {
    expect(errText('boom')).toBe('boom');
    expect(errText(new Error('网络断了'))).toBe('网络断了');
  });
});

describe('errMsg', () => {
  it('有登记 kind 的补一行下一步指引', () => {
    expect(errMsg(appError('conflict', '「x」已存在'))).toBe(
      '「x」已存在\n存在冲突（同名或未归档等），先处理冲突再试',
    );
    expect(errMsg(appError('client_running', '客户端还开着'))).toBe(
      '客户端还开着\n客户端没能自动关闭，请从托盘退出后再试',
    );
  });

  it('未登记 kind 只报 message，不加废话', () => {
    expect(errMsg(appError('cancelled', '用户取消'))).toBe('用户取消');
  });

  it('调用方可以覆盖指引（按动作定制话术）', () => {
    expect(errMsg(appError('conflict', '有冲突'), { conflict: '先去会话页归档' })).toBe(
      '有冲突\n先去会话页归档',
    );
  });

  it('kind 解析失败时退回纯 message', () => {
    expect(errMsg('不是 AppError')).toBe('不是 AppError');
    expect(errKind('不是 AppError')).toBeNull();
  });
});

// P0-1：国际版在未开系统代理时必然超时，而浏览器能上网——用户会认定「软件坏了」。
// io 是最杂的一类错误（超时 / 磁盘满 / 权限），必须按原文分流，不能一律劝人开代理。
describe('errMsg 的网络与本机指引', () => {
  it('io 里的超时类原文指向系统代理与网络自检', () => {
    for (const message of ['请求超时', 'operation timed out', '连接被拒绝', 'dns error']) {
      const text = errMsg(appError('io', message));
      expect(text.startsWith(message)).toBe(true);
      expect(text).toContain('代理');
      expect(text).toContain('自检');
    }
  });

  it('io 里的磁盘/权限类原文不劝人开代理', () => {
    const text = errMsg(appError('io', '磁盘满了'));
    expect(text).toContain('磁盘');
    expect(text).not.toContain('代理');
  });

  it('调用方给的 io 指引优先于内置分流', () => {
    expect(errMsg(appError('io', '请求超时'), { io: '先看客户端是否还开着' })).toBe(
      '请求超时\n先看客户端是否还开着',
    );
  });

  it('凭据与档案类错误各自指向下一步', () => {
    expect(errMsg(appError('auth_expired', '登录态过期'))).toContain('重新登录');
    expect(errMsg(appError('session_state_missing', '没有档案'))).toContain('登录一次');
    expect(errMsg(appError('profile_mismatch', '档案不匹配'))).toContain('重新登录');
    expect(errMsg(appError('crypto', '解密失败'))).toContain('Windows');
    expect(errMsg(appError('crypto', '解密失败'))).toContain('当前用户');
  });

  it('平台限制与写盘失败不甩锅给网络', () => {
    expect(errMsg(appError('unsupported', '不支持'))).toContain('平台');
    expect(errMsg(appError('write_failed', '写不进去'))).toContain('磁盘');
    expect(errMsg(appError('provider', '接口异常'))).toContain('刷新凭据');
  });
});

// 执行动作的详情：要能回答「当时到底传了什么、回来的是什么」，而不是一串占位符。
describe('argsSummary', () => {
  it('数组摊开列出元素，不再退化成「N 项」', () => {
    expect(argsSummary({ iids: ['workbuddy:a', 'qoder:b'] })).toBe('iids=[workbuddy:a, qoder:b]');
  });

  it('超长数组只列前 8 项并说明总数', () => {
    const many = Array.from({ length: 11 }, (_, i) => `k${i}`);
    const text = argsSummary({ keys: many });
    expect(text).toContain('k0');
    expect(text).toContain('共 11 项');
    expect(text).not.toContain('k9');
  });

  it('凭据字段一律打码', () => {
    expect(argsSummary({ token: 'sk-123', secret: 's', iid: 'workbuddy:a' })).toBe(
      'token=*** secret=*** iid=workbuddy:a',
    );
  });

  it('空值跳过，对象按 JSON 展开', () => {
    expect(argsSummary({ a: null, b: '', c: { k: 1 } })).toBe('c={"k":1}');
  });
});

describe('resultSummary', () => {
  it('字符串原样、数字转字符串、空返回空', () => {
    expect(resultSummary('已删除 3 条')).toBe('已删除 3 条');
    expect(resultSummary(11)).toBe('11');
    expect(resultSummary(null)).toBe('');
    expect(resultSummary(undefined)).toBe('');
  });

  it('对象转 JSON，过长时标注原长度', () => {
    expect(resultSummary({ scanned: 11, failed: 0 })).toBe('{"scanned":11,"failed":0}');
    const long = resultSummary('x'.repeat(500));
    expect(long).toContain('共 500 字');
  });
});

// 传输回归：命令面必须经 ipc.call 走 HTTP /cmd 并带安全头。
// 这是唯一传输路径——头漏了会被后端安全面 403，
// 健康报告失败、列表为空、导出报错都会是它。
describe('命令面传输', () => {
  const stubHttp = (data: unknown) => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true, data }), { status: 200 }),
    );
    vi.stubGlobal('fetch', fetchMock);
    return fetchMock;
  };

  it('queryCmd 走 HTTP /cmd（带安全头）', async () => {
    const fetchMock = stubHttp('2.0.0');
    await expect(queryCmd('app_version')).resolves.toBe('2.0.0');
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('/cmd/app_version');
    expect(init?.headers).toMatchObject({ 'x-ai-manager-web': '1' });
    vi.unstubAllGlobals();
  });

  it('runCmd 同样走 HTTP /cmd，成功值不被吞', async () => {
    stubHttp(11);
    await expect(runCmd('account_list')).resolves.toBe(11);
    vi.unstubAllGlobals();
  });

  // P0-2：toast 正文只给人话 + 下一步，kind / 参数 / 原文这类排障字段折进「详情」。
  // 新手读得懂第一行，专业用户点开还能拿到全部证据（日志里同样留全量）。
  it('失败 toast 正文不含排障字段，证据放进 detail', async () => {
    const { useToasts } = await import('@/components/ui/toast');
    useToasts.setState({ items: [] });
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        new Response(JSON.stringify({ ok: false, error: appError('io', '请求超时') }), {
          status: 200,
        }),
      ),
    );
    await expect(runCmd('account_scan')).resolves.toBeUndefined();
    const [item] = useToasts.getState().items;
    expect(item.text).toContain('请求超时');
    expect(item.text).not.toContain('kind=');
    expect(item.text).not.toContain('原文=');
    expect(item.detail).toContain('kind=io');
    expect(item.detail).toContain('原文=请求超时');
    vi.unstubAllGlobals();
  });
});
