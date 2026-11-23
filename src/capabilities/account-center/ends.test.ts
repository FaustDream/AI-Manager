// 「客户端的登录状态」：CodeBuddy 系两个端现在登的是谁，没装 / 没登录也要占一行。
// 识别靠 uid（只在内部比对，界面不出现）：同一个号在列表里显示同一个用户名。
import { describe, expect, it } from 'vitest';
import type { AccountView, CodebuddyTargetStatus } from '@/lib/types';
import { endRows, endStateLabel, endSwitchCandidates, type EndRow } from './ends';

const row = (over: Partial<AccountView> = {}) =>
  ({
    iid: 'workbuddy:u1',
    platform: 'workbuddy',
    platform_label: 'WorkBuddy',
    name: '张三',
    ...over,
  }) as unknown as AccountView;

const target = (over: Partial<CodebuddyTargetStatus> = {}) =>
  ({
    key: 'software',
    label: 'CodeBuddy IDE',
    installed: true,
    current_name: '客户端里写的昵称',
    current_uid: 'u1',
    edition: 'cn',
    ...over,
  }) as CodebuddyTargetStatus;

describe('endRows（端状态行）', () => {
  it('uid 相同就是同一个号：名字取账号库那一行，并挂上归属', () => {
    const [end] = endRows([row({ name: '张三（别名）' })], [target()]);
    expect(end).toEqual({
      key: 'software',
      label: 'CodeBuddy IDE',
      name: '张三（别名）',
      clientName: '客户端里写的昵称',
      ownerIid: 'workbuddy:u1',
      state: 'signed_in',
      intl: false,
    });
  });

  it('库里没有这个号时退回客户端昵称，不归属任何账号行', () => {
    const [end] = endRows([row()], [target({ current_uid: 'u9' })]);
    expect(end.name).toBeNull();
    expect(end.clientName).toBe('客户端里写的昵称');
    expect(end.ownerIid).toBeNull();
    expect(end.state).toBe('signed_in');
  });

  it('uid 不同不会串到别的账号上', () => {
    const ends = endRows([row({ iid: 'workbuddy:u2', name: '李四' })], [target()]);
    expect(ends[0].ownerIid).toBeNull();
    expect(ends[0].name).toBeNull();
  });

  it('没登录的端照样占一行，状态是「未登录」', () => {
    const [end] = endRows([row()], [target({ current_uid: null, current_name: null })]);
    expect(end.state).toBe('signed_out');
    expect(end.ownerIid).toBeNull();
    expect(endStateLabel(end)).toBe('未登录');
  });

  it('没装的端也占一行，状态是「未安装」', () => {
    const [end] = endRows([row()], [target({ installed: false, current_uid: null, current_name: null })]);
    expect(end.state).toBe('absent');
    expect(endStateLabel(end)).toBe('未安装');
  });

  it('登在国际版的端保留展示，但标成不可写', () => {
    const [end] = endRows([row()], [target({ edition: 'intl' })]);
    expect(end.intl).toBe(true);
    // 国际版账号不参与归属：uid 撞车也不是同一个号
    expect(end.ownerIid).toBeNull();
  });

  it('国际版账号行不参与归属比对', () => {
    const [end] = endRows([row({ iid: 'workbuddy:u1@intl' })], [target()]);
    expect(end.ownerIid).toBeNull();
  });

  it('两个端各占一行，顺序跟后端给的 targets 一致', () => {
    const ends = endRows(
      [row()],
      [target(), target({ key: 'idea', label: 'CodeBuddy 插件', current_uid: 'u2' })],
    );
    expect(ends.map((end) => end.key)).toEqual(['software', 'idea']);
  });
});

describe('endSwitchCandidates（这一端能切到哪些号）', () => {
  const end = (over: Partial<EndRow> = {}): EndRow => ({
    key: 'software',
    label: 'CodeBuddy IDE',
    name: '张三',
    clientName: '张三',
    ownerIid: 'workbuddy:u1',
    state: 'signed_in',
    intl: false,
    ...over,
  });

  it('只给同平台的其它号：这一端已经登着的那个不重复列出', () => {
    const rows = [
      row({ iid: 'workbuddy:u1', name: '张三' }),
      row({ iid: 'workbuddy:u2', name: '李四' }),
    ];
    const got = endSwitchCandidates(rows, end());
    expect(got.map((r) => r.iid)).toEqual(['workbuddy:u2']);
  });

  it('国际版不算同平台：后端只认国内版档案，列出来也切不过去', () => {
    const rows = [
      row({ iid: 'workbuddy:u1' }),
      row({ iid: 'workbuddy:u2@intl', name: '国际号' }),
    ];
    expect(endSwitchCandidates(rows, end())).toEqual([]);
  });

  it('别的平台不进候选；缺客户端档案的号仍然列出（由界面标注原因）', () => {
    const rows = [
      row({ iid: 'workbuddy:u2', name: '李四' }),
      row({ iid: 'zcode:z1', platform: 'zcode', name: 'ZCode 号' }),
      row({ iid: 'workbuddy:u3', name: '王五' }),
    ];
    const got = endSwitchCandidates(rows, end());
    expect(got.map((r) => r.iid)).toEqual(['workbuddy:u2', 'workbuddy:u3']);
  });

  it('端还没登录（没有归属）时也能选到全部国内号', () => {
    const rows = [
      row({ iid: 'workbuddy:u1', name: '张三' }),
      row({ iid: 'workbuddy:u2', name: '李四' }),
    ];
    const got = endSwitchCandidates(rows, end({ ownerIid: null, state: 'signed_out' }));
    expect(got.map((r) => r.iid)).toEqual(['workbuddy:u1', 'workbuddy:u2']);
  });
});

describe('endStateLabel（状态短文案）', () => {
  it('已登录优先用账号库的显示名，其次客户端昵称', () => {
    const base: EndRow = {
      key: 'idea',
      label: 'CodeBuddy 插件',
      name: '凌致',
      clientName: '客户端昵称',
      ownerIid: 'workbuddy:u1',
      state: 'signed_in',
      intl: false,
    };
    expect(endStateLabel(base)).toBe('凌致');
    expect(endStateLabel({ ...base, name: null })).toBe('客户端昵称');
    expect(endStateLabel({ ...base, name: null, clientName: null })).toBe('已登录');
  });
});
