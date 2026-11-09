// IPC 的类型化包装（§6.2）：AppError 统一在这里转成可展示文案，
// 页面不允许自己拼错误处理——v1 的 _error_text 分散各页是同类问题的根源。
//
// 传输分流（网页调试模式）：桌面 WebView 里走 Tauri invoke；普通浏览器里
// 走 vite 代理到 `ai-manager-web` 的 HTTP（POST /cmd/<命令名>）。两边的
// 错误形状一致：后端命令返回 Result<_, String>，字符串里是序列化的 AppError。
import { invoke } from '@tauri-apps/api/core';

export interface AppError {
  kind: string;
  message: string;
  detail?: string | null;
}

export function isAppError(value: unknown): value is AppError {
  return typeof value === 'object' && value !== null && 'kind' in value && 'message' in value;
}

/** 当前是否跑在 Tauri WebView 里。网页调试模式（纯浏览器）为 false。 */
export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

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

/** 命令入参键 camelCase（前端习惯）→ snake_case（Rust 参数名）。
 *  只转顶层：嵌套对象（Filter、Schedule 等载荷类型）的字段名本就由 serde 决定。 */
function snakeKey(key: string): string {
  return key.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);
}

/** 网页调试模式：POST /cmd/<命令名>，回包 {ok, data} 或 {ok, error}。
 *  失败时把 error 字符串 throw 出去——与 invoke 的 reject 形状一致，
 *  上层 errText/errMsg 不用关心传输差异。 */
async function httpCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const body: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(args ?? {})) {
    body[snakeKey(key)] = value;
  }
  let res: Response;
  try {
    res = await fetch(`/cmd/${cmd}`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        // 必须带：后端安全面要求这个自定义头（强制跨站请求先走 CORS 预检）
        'x-ai-manager-web': '1',
      },
      body: JSON.stringify(body),
    });
  } catch {
    throw `命令 ${cmd} 发不出去：网页调试壳没在运行（先执行 cargo run --bin ai-manager-web）`;
  }
  const reply = (await res.json().catch(() => null)) as {
    ok: boolean;
    data?: T;
    error?: string;
  } | null;
  if (!reply) {
    throw `命令 ${cmd} 的响应不是合法 JSON（HTTP ${res.status}）`;
  }
  if (!reply.ok) {
    throw reply.error ?? `命令 ${cmd} 失败（HTTP ${res.status}）`;
  }
  return reply.data as T;
}

export function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return inTauri ? invoke<T>(cmd, args) : httpCall<T>(cmd, args);
}
