// C1 的错误话术：errMsg（kind→下一步指引）与 errText（取 message）都是纯函数。
import { describe, expect, it } from 'vitest';
import { errKind, errText } from './ipc';
import { errMsg } from './cmd';

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
    expect(errMsg(appError('io', '磁盘满了'))).toBe('磁盘满了');
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
