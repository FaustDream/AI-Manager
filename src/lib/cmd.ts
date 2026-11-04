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
  ArchiveReport,
  ClaimOutcome,
  ClaimPlan,
  CreditSnapshot,
  DeployReport,
  Facets,
  Filter,
  HubSnapshot,
  ImportReport,
  LogLine,
  LogFilter,
  LoginStart,
  Manifest,
  MigrationReport,
  Page,
  Preview,
  RestoreOutcome,
  RestoreReport,
  RoundResult,
  ScanReport,
  SessionRow,
  Snapshot,
  SpaceEntry,
  SpaceReport,
  SwitchResult,
  TaskView,
} from './types';

/** 每个命令一行：JS 入参。省略 `State`/`AppHandle` 这类注入参数。 */
export interface CmdArgs {
  app_version: void;
  ui_ready: void;
  app_quit: void;
  window_show: void;
  window_hide: void;
  tray_set_autostart: { enabled: boolean };
  settings_get: { key: string };
  settings_set: { key: string; value: unknown };
  task_list: void;
  task_cancel: { taskId: number };
  task_clear_finished: void;
  account_list: void;
  account_reorder: { iids: string[] };
  account_delete: { iid: string };
  account_scan: void;
  account_login_start: { platform: string };
  account_switch: { iid: string };
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
  log_level: { level: string };
  maintenance_space: void;
  maintenance_trash: void;
  maintenance_purge_trash: { tag: string | null };
  maintenance_open_data: void;
  migrate_verify: void;
  migrate_archive: void;
  autostart_get: void;
  autostart_set: { enabled: boolean };
}

/** 每个命令一行：成功返回值（Result<T, _> 只留 T）。 */
export interface CmdReturn {
  app_version: string;
  ui_ready: string;
  app_quit: void;
  window_show: void;
  window_hide: void;
  tray_set_autostart: void;
  settings_get: unknown;
  settings_set: void;
  task_list: TaskView[];
  task_cancel: boolean;
  task_clear_finished: number;
  account_list: AccountView[];
  account_reorder: void;
  account_delete: boolean;
  account_scan: ScanReport;
  account_login_start: LoginStart;
  account_switch: SwitchResult;
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
  session_scan: number;
  session_page: Page<SessionRow>;
  session_preview: Preview;
  session_running_clients: string[];
  session_delete: number;
  session_backup: Snapshot;
  session_backups: Snapshot[];
  session_trash: Manifest[];
  session_trash_restore: RestoreReport;
  session_trash_purge: number;
  session_keys: string[];
  session_facets: Facets;
  session_archive: number;
  session_purge: number;
  session_retention: number;
  session_retention_set: number;
  checkin_credits: [string, CreditSnapshot][];
  checkin_detail: AccountDetail;
  checkin_run: ActionResult;
  checkin_batch: number;
  daily_state: RoundResult | null;
  daily_run_now: number;
  zcode_claim_plans: ClaimPlan[];
  zcode_claim_run: ClaimOutcome;
  log_query: LogLine[];
  log_export: string;
  log_level: void;
  maintenance_space: SpaceReport;
  maintenance_trash: SpaceEntry[];
  maintenance_purge_trash: number;
  maintenance_open_data: void;
  migrate_verify: MigrationReport;
  migrate_archive: ArchiveReport;
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

/** 查询入口：失败时抛错（React Query 吃到错误进 error 态），不吞不吐 toast。 */
export function queryCmd<K extends CmdName>(cmd: K, ...rest: CmdRest<K>): Promise<CmdReturn[K]> {
  const [args] = rest;
  return invoke<CmdReturn[K]>(cmd, args as Record<string, unknown> | undefined);
}

/** 命令的唯一入口：失败时按 opts 处理（缺省 toast），返回 undefined 与成功值区分。 */
export async function runCmd<K extends CmdName>(
  cmd: K,
  ...rest: CmdRest<K>
): Promise<CmdReturn[K] | undefined> {
  const [args, opts] = rest;
  try {
    return await invoke<CmdReturn[K]>(cmd, args as Record<string, unknown> | undefined);
  } catch (e) {
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
