// 命令调用面（架构评审 C1）：命令名单与形状在这一张表里钉死，
// 错误 kind→话术、忙碌态都收在这里——页面只表达意图，不碰 invoke/toast 的细节。
// 名单与 src-tauri 侧 generate_handler! 一一对应：改命令先改这张表（tsc 会红）。
import { useCallback, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from '@/components/ui/toast';
import { errKind, errText } from './ipc';
import type {
  AccountDetail,
  AccountView,
  ActionResult,
  AddOutcome,
  ClaimOutcome,
  ClaimPlan,
  CodebuddyTargetStatus,
  CreditSnapshot,
  DeleteReport,
  DeployReport,
  Facets,
  Filter,
  HubSnapshot,
  ImportReport,
  LogLine,
  LogFilter,
  LoginStart,
  Manifest,
  Page,
  Preview,
  PurgeReport,
  RestoreOutcome,
  RestoreReport,
  RoundResult,
  Schedule,
  ScanReport,
  SessionRow,
  Snapshot,
  SpaceEntry,
  SpaceReport,
  SwitchResult,
} from './types';

/** 每个命令一行：JS 入参。省略 `State`/`AppHandle` 这类注入参数。 */
export interface CmdArgs {
  ui_ready: void;
  app_quit: void;
  settings_get: { key: string };
  settings_set: { key: string; value: unknown };
  account_list: void;
  account_reorder: { iids: string[] };
  account_delete: { iid: string };
  account_scan: void;
  account_login_start: { platform: string };
  account_switch: { iid: string };
  account_switch_codebuddy: { iid: string; target: 'software' | 'idea' };
  account_codebuddy_targets: void;
  account_restore_sessions: { iid: string };
  account_uid: { iid: string };
  account_current: void;
  account_add_prepare: { platform: string };
  account_open_client: { platform: string };
  open_external: { url: string };
  app_show_window: void;
  hub_snapshot: void;
  hub_skill_import_many: { target: string; names: string[] };
  hub_skill_deploy: { name: string; targets: string[]; overwrite?: boolean | null };
  hub_skill_remove: { name: string; targets: string[] };
  hub_skill_import: { target: string; name: string };
  hub_mcp_import: { text: string; targets: string[] };
  hub_mcp_remove: { name: string; targets: string[] };
  hub_mcp_deploy: { name: string; targets: string[] };
  session_scan: void;
  session_page: { cursor: string | null; limit: number; filter: Filter };
  session_preview: { key: string };
  session_running_clients: { keys: string[] };
  session_delete: { keys: string[] };
  session_backup: void;
  session_backups: void;
  session_trash: void;
  session_trash_restore: { tag: string };
  session_trash_purge: { tag: string | null };
  session_keys: { filter: Filter };
  session_facets: void;
  session_archive: { keys: string[]; on: boolean };
  session_purge: { keys: string[] };
  session_retention: void;
  session_retention_set: { days: number };
  checkin_credits: { iids: string[] };
  checkin_detail: { iid: string };
  checkin_run: { iid: string };
  checkin_batch: { iids: string[] };
  daily_state: void;
  daily_run_now: void;
  zcode_claim_plans: { iid: string };
  zcode_claim_run: { iid: string; planId: string };
  log_query: { filter: LogFilter };
  log_export: { path: string | null; filter: LogFilter };
  log_action: { action: string; trigger: string; ok: boolean; ms: number; detail: string };
  log_ui: {
    level: string;
    action: string | null;
    message: string;
    evidence: string | null;
  };
  schedule_list: void;
  schedule_save: { input: Schedule };
  schedule_delete: { id: string };
  schedule_run: { id: string };
  log_level: { level: string };
  maintenance_space: void;
  maintenance_trash: void;
  maintenance_purge_trash: { tag: string | null };
  maintenance_open_data: void;
  autostart_get: void;
  autostart_set: { enabled: boolean };
}

/** 每个命令一行：成功返回值（Result<T, _> 只留 T）。 */
export interface CmdReturn {
  ui_ready: string;
  app_quit: void;
  settings_get: unknown;
  settings_set: void;
  account_list: AccountView[];
  account_reorder: void;
  account_delete: boolean;
  account_scan: ScanReport;
  account_login_start: LoginStart;
  account_switch: SwitchResult;
  account_switch_codebuddy: SwitchResult;
  account_codebuddy_targets: CodebuddyTargetStatus[];
  account_restore_sessions: RestoreOutcome;
  account_uid: string;
  account_current: string[];
  account_add_prepare: AddOutcome;
  account_open_client: boolean;
  open_external: void;
  app_show_window: void;
  hub_snapshot: HubSnapshot;
  hub_skill_import_many: ImportReport;
  hub_skill_deploy: DeployReport;
  hub_skill_remove: DeployReport;
  hub_skill_import: { name: string };
  hub_mcp_import: { names: string[]; report: DeployReport };
  hub_mcp_remove: DeployReport;
  hub_mcp_deploy: DeployReport;
  session_scan: ScanReport;
  session_page: Page<SessionRow>;
  session_preview: Preview;
  session_running_clients: string[];
  session_delete: DeleteReport;
  session_backup: Snapshot;
  session_backups: Snapshot[];
  session_trash: Manifest[];
  session_trash_restore: RestoreReport;
  session_trash_purge: number;
  session_keys: string[];
  session_facets: Facets;
  session_archive: number;
  session_purge: PurgeReport;
  session_retention: number;
  session_retention_set: number;
  checkin_credits: [string, CreditSnapshot][];
  checkin_detail: AccountDetail;
  checkin_run: ActionResult;
  checkin_batch: RoundResult;
  daily_state: RoundResult | null;
  daily_run_now: RoundResult;
  zcode_claim_plans: ClaimPlan[];
  zcode_claim_run: ClaimOutcome;
  log_query: LogLine[];
  log_export: string;
  log_action: void;
  log_ui: void;
  schedule_list: Schedule[];
  schedule_save: Schedule;
  schedule_delete: void;
  schedule_run: string;
  log_level: void;
  maintenance_space: SpaceReport;
  maintenance_trash: SpaceEntry[];
  maintenance_purge_trash: number;
  maintenance_open_data: void;
  autostart_get: boolean;
  autostart_set: void;
}

export type CmdName = keyof CmdReturn;

interface CmdOpts {
  /** 自己处理错误（登录向导要把错误写进内部状态）；缺省统一 toast */
  onError?: (e: unknown) => void;
  /** 按动作覆盖 kind→下一步指引 */
  hint?: Record<string, string>;
}

type CmdRest<K extends CmdName> = CmdArgs[K] extends void
  ? [args?: undefined, opts?: CmdOpts]
  : [args: CmdArgs[K], opts?: CmdOpts];

/** kind→下一步指引：错误话术的第二行，一处维护。 */
const KIND_HINT: Record<string, string> = {
  client_running: '客户端没能自动关闭，请从托盘退出后再试',
  conflict: '存在冲突（同名或未归档等），先处理冲突再试',
};

/** 错误话术：message 为主体，kind 有指引时补一行下一步。 */
export function errMsg(e: unknown, hint?: Record<string, string>): string {
  const guide = { ...KIND_HINT, ...hint }[errKind(e) ?? ''];
  const text = errText(e);
  return guide ? `${text}\n${guide}` : text;
}

/** 命令 → 人话动作名。执行记录是给人看的，不能只留 `session_purge` 这种键。 */
const CMD_LABELS: Record<string, string> = {
  ui_ready: '界面就绪',
  app_quit: '退出应用',
  app_show_window: '显示主窗口',
  settings_get: '读取设置',
  settings_set: '修改设置',
  account_list: '读取账号列表',
  account_reorder: '调整账号顺序',
  account_delete: '删除账号',
  account_scan: '扫描账号',
  account_login_start: '开始添加账号',
  account_switch: '切换登录',
  account_switch_codebuddy: '切换 CodeBuddy 登录',
  account_codebuddy_targets: '读取 CodeBuddy 登录状态',
  account_restore_sessions: '恢复会话归属',
  account_uid: '复制账号 UID',
  account_current: '读取当前登录',
  account_add_prepare: '打开客户端登录页',
  account_open_client: '打开客户端',
  open_external: '打开外部链接',
  hub_snapshot: '读取技能与 MCP',
  hub_skill_import: '导入技能',
  hub_skill_import_many: '批量导入技能',
  hub_skill_deploy: '部署技能',
  hub_skill_remove: '移除技能',
  hub_mcp_import: '导入 MCP',
  hub_mcp_deploy: '部署 MCP',
  hub_mcp_remove: '移除 MCP',
  session_scan: '扫描会话',
  session_page: '读取会话列表',
  session_preview: '预览会话正文',
  session_running_clients: '检查运行中的客户端',
  session_delete: '删除会话',
  session_purge: '彻底删除会话',
  session_backup: '备份会话库',
  session_backups: '读取会话快照',
  session_trash: '读取会话回收站',
  session_trash_restore: '恢复回收站批次',
  session_trash_purge: '清空回收站批次',
  session_keys: '读取全部会话键',
  session_facets: '读取会话筛选值',
  session_archive: '归档会话',
  session_retention: '读取自动归档天数',
  session_retention_set: '设置自动归档天数',
  checkin_credits: '刷新积分',
  checkin_detail: '读取账号详情',
  checkin_run: '签到',
  checkin_batch: '批量签到',
  daily_state: '读取轮询结果',
  daily_run_now: '手动跑一轮轮询',
  zcode_claim_plans: '拉取可领活动',
  zcode_claim_run: '领取活动',
  log_query: '查询日志',
  log_export: '导出日志',
  log_level: '调整日志级别',
  maintenance_space: '读取空间占用',
  maintenance_trash: '读取维护回收站',
  maintenance_purge_trash: '清空维护回收站',
  maintenance_open_data: '打开数据目录',
  autostart_get: '读取开机自启',
  autostart_set: '设置开机自启',
  schedule_list: '读取定时任务',
  schedule_save: '保存定时任务',
  schedule_delete: '删除定时任务',
  schedule_run: '立即执行定时任务',
  log_ui: '界面日志',
};

/**
 * 会进「执行记录」的动作：**只收改变状态或需要事后追责的**。
 *
 * 读列表、开窗口、复制 UID 这类不记——执行记录是给人扫的，塞满无意义动作
 * 等于没有记录。读失败仍会进日志（见 `logUiFailure`），只是不占执行记录的位置。
 */
const KEY_ACTIONS = new Set<string>([
  'app_quit',
  'account_scan',
  'account_delete',
  'account_switch',
  'account_switch_codebuddy',
  'account_restore_sessions',
  'account_reorder',
  'account_add_prepare',
  'session_scan',
  'session_delete',
  'session_purge',
  'session_archive',
  'session_backup',
  'session_trash_restore',
  'session_trash_purge',
  'session_retention_set',
  'checkin_credits',
  'zcode_claim_run',
  'hub_skill_import',
  'hub_skill_import_many',
  'hub_skill_deploy',
  'hub_skill_remove',
  'hub_mcp_import',
  'hub_mcp_deploy',
  'hub_mcp_remove',
  'settings_set',
  'autostart_set',
  'log_level',
  'log_export',
  'maintenance_purge_trash',
  'schedule_save',
  'schedule_delete',
  'schedule_run',
]);

/** 这些参数是凭据，进日志前必须打码——执行记录是长期留存的。 */
const SECRET_KEY = /token|secret|password|passwd|cookie|credential|code/i;

/** 单个值、整条详情的上限。日志要能回答「当时到底传了什么」，不是留个占位符。 */
const MAX_VALUE = 160;
const MAX_DETAIL = 900;
const MAX_RESULT = 400;
/** 报错原文单独放宽：排障靠的就是这句原文，截断到看不清等于没记。 */
const MAX_ERROR = 1200;

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max)}…（共 ${text.length} 字）` : text;
}

/**
 * 失败详情：错误类别 + 参数 + 原文。
 *
 * 只记一句「失败」是没用的——事后要判断「是参数不对、客户端没关、还是网络挂了」，
 * 靠的就是 `kind` 和原文。凭据仍然走 `argsSummary` 打码。
 */
export function failureSummary(error: unknown, args?: unknown): string {
  const parts = [
    `kind=${errKind(error) ?? 'unknown'}`,
    argsSummary(args),
    `原文=${clip(errText(error), MAX_ERROR)}`,
  ];
  return parts.filter(Boolean).join(' · ');
}

/** 值转人读：数组摊开列出来，对象直接 JSON，不再退化成「[N 项]」。 */
function describeValue(value: unknown): string {
  if (Array.isArray(value)) {
    const head = value.slice(0, 8).map((item) => String(item));
    const more = value.length > 8 ? ` …共 ${value.length} 项` : '';
    return `[${head.join(', ')}${more}]`;
  }
  if (value && typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/** 参数摘要：脱敏 + 摊平。宁可长一点，也别让人对着一串 `[11 项]` 猜。 */
export function argsSummary(args: unknown): string {
  if (!args || typeof args !== 'object') return '';
  const parts: string[] = [];
  for (const [key, value] of Object.entries(args as Record<string, unknown>)) {
    if (value === null || value === undefined || value === '') continue;
    const text = SECRET_KEY.test(key) ? '***' : clip(describeValue(value), MAX_VALUE);
    parts.push(`${key}=${text}`);
  }
  return clip(parts.join(' '), MAX_DETAIL);
}

/** 返回值摘要：成功也要留下「做成了什么」，否则只剩一句「成功」。 */
export function resultSummary(result: unknown): string {
  if (result === undefined || result === null) return '';
  if (typeof result === 'string') return clip(result, MAX_RESULT);
  if (typeof result === 'number' || typeof result === 'boolean') return String(result);
  return clip(JSON.stringify(result), MAX_RESULT);
}

/**
 * 记一条执行记录。**失败也绝不能因为日志把业务带崩**，所以整段吞掉错误。
 *
 * 走独立命令而不是让后端每个命令自己写：命令有几十条，只有前端知道
 * 「用户点了什么、参数是什么、等了多久、回来的是什么」，后端各写各的必然漏。
 */
function logAction(
  cmd: string,
  args: unknown,
  ok: boolean,
  ms: number,
  error?: string,
  result?: unknown,
) {
  if (!KEY_ACTIONS.has(cmd)) return;
  const label = CMD_LABELS[cmd] ?? cmd;
  const detail = [
    label,
    argsSummary(args),
    ok ? resultSummary(result) : `失败：${error ?? '未知错误'}`,
  ]
    .filter(Boolean)
    .join(' · ');
  void invoke('log_action', { action: cmd, trigger: 'user', ok, ms, detail }).catch(() => {});
}

/**
 * 失败留痕：**所有命令**都要记，不只是关键动作。
 *
 * 以前这里只记「读失败」且是 warn，非关键动作的失败干脆没有任何记录 ——
 * 界面弹一句 toast 就消失了，事后完全查不出「刚才那个报错是什么」。
 * 现在统一记 error，并带上 `action`（哪个命令）与 `evidence`（kind + 原文）。
 */
function logUiFailure(cmd: string, error: unknown, args?: unknown) {
  const label = CMD_LABELS[cmd] ?? cmd;
  void invoke('log_ui', {
    level: 'error',
    action: cmd,
    message: `${label}失败（${cmd}）`,
    evidence: failureSummary(error, args),
  }).catch(() => {});
}

/** 查询入口：失败时抛错（React Query 吃到错误进 error 态），不吞不吐 toast。 */
export async function queryCmd<K extends CmdName>(
  cmd: K,
  ...rest: CmdRest<K>
): Promise<CmdReturn[K]> {
  const [args] = rest;
  try {
    return await invoke<CmdReturn[K]>(cmd, args as Record<string, unknown> | undefined);
  } catch (e) {
    logUiFailure(cmd, e, args);
    throw e;
  }
}

/** 命令的唯一入口：失败时按 opts 处理（缺省 toast），返回 undefined 与成功值区分。 */
export async function runCmd<K extends CmdName>(
  cmd: K,
  ...rest: CmdRest<K>
): Promise<CmdReturn[K] | undefined> {
  const [args, opts] = rest;
  const started = performance.now();
  try {
    const result = await invoke<CmdReturn[K]>(cmd, args as Record<string, unknown> | undefined);
    // 成功也要记：用户事后要能回答「我点过什么、它做成了什么」
    logAction(cmd, args, true, Math.round(performance.now() - started), undefined, result);
    return result;
  } catch (e) {
    // 关键动作进「执行记录」，其余至少进日志——两者都不记的话，
    // 用户点完只看到一句 toast，事后没有任何痕迹
    if (KEY_ACTIONS.has(cmd)) {
      logAction(cmd, args, false, Math.round(performance.now() - started), failureSummary(e, args));
    } else {
      logUiFailure(cmd, e, args);
    }
    if (opts?.onError) opts.onError(e);
    else toast(errMsg(e, opts?.hint));
    return undefined;
  }
}

/** 忙碌态：同一时刻只跑一个动作，key 用来让对应按钮进 loading。 */
export function useBusy() {
  const [busy, setBusy] = useState<string | null>(null);
  const ref = useRef<string | null>(null);
  const run = useCallback(async (key: string, work: () => Promise<void>): Promise<void> => {
    if (ref.current) return;
    ref.current = key;
    setBusy(key);
    try {
      await work();
    } finally {
      ref.current = null;
      setBusy(null);
    }
  }, []);
  return { busy, run };
}
