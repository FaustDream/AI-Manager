// IPC 的类型化包装（§6.2）：AppError 统一在这里转成可展示文案，
// 页面不允许自己拼错误处理——v1 的 _error_text 分散各页是同类问题的根源。
import { invoke } from '@tauri-apps/api/core';

export interface AppError {
  kind: string;
  message: string;
  detail?: string | null;
}

export function isAppError(value: unknown): value is AppError {
  return typeof value === 'object' && value !== null && 'kind' in value && 'message' in value;
}

/** 后端命令返回 Result<_, String>，字符串里是序列化的 AppError。 */
export function errText(e: unknown): string {
  if (typeof e === 'string') {
    try {
      const parsed = JSON.parse(e) as AppError;
      if (isAppError(parsed)) return parsed.message;
    } catch {
      /* 不是 JSON 就原样返回 */
    }
    return e;
  }
  if (isAppError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** 错误类别：界面按 kind 决定话术（client_running / conflict 要给下一步指引）。 */
export function errKind(e: unknown): string | null {
  if (typeof e === 'string') {
    try {
      const parsed = JSON.parse(e) as AppError;
      if (isAppError(parsed)) return parsed.kind;
    } catch {
      /* 原样返回 null */
    }
  }
  if (isAppError(e)) return e.kind;
  return null;
}

export function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, args);
}
