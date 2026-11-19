// 接口反代：把豆包账号暴露成本机 OpenAI / Anthropic / Responses 兼容接口。
//
// 从 设置 → 常规 迁出后的一级标签页。这里只做「服务」该做的事：
// 状态与启停、地址/端口/Key、运行统计与最近调用、客户端接入示例、模型目录、自检。
// 端口/Key/模型的读写仍走命令面（model_api_*），页面不直接 invoke。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { Card, Notice, Tag } from '@/components/ui';
import { Button, cn } from '@/components/ui/button';
import { confirmAction } from '@/components/ui/confirm';
import { Input, SkeletonBlock, StateBlock } from '@/components/ui/form';
import { toast } from '@/components/ui/toast';
import { queryCmd, runCmd } from '@/lib/cmd';
import { fmtMs, fmtTime } from '@/lib/format';
import { errText } from '@/lib/ipc';
import { navigate } from '@/lib/nav';
import type { ModelApiInfo, ModelApiProbe, ModelApiStats, ModelEntry } from '@/lib/types';

/** 运行时长：超过一天只说天/小时，秒级只在刚启动时有意义。 */
function fmtUptime(secs: number): string {
  if (!secs) return '—';
  if (secs < 60) return `${secs} 秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)} 分`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时 ${Math.floor((secs % 3600) / 60)} 分`;
  return `${Math.floor(secs / 86400)} 天 ${Math.floor((secs % 86400) / 3600)} 小时`;
}

/** 调用路径 → 人话。统计表里只留接口名，排障时不用再对照文档。 */
const ENDPOINT_LABEL: Record<string, string> = {
  '/v1/chat/completions': 'OpenAI Chat',
  '/v1/messages': 'Anthropic Messages',
  '/v1/messages/count_tokens': 'Token 估算',
  '/v1/responses': 'OpenAI Responses',
  '/responses': 'OpenAI Responses',
  '/v1/models': '模型目录',
};

export default function ModelApiPage() {
  const qc = useQueryClient();
  const infoQ = useQuery<ModelApiInfo>({
    queryKey: ['model-api-info'],
    queryFn: () => queryCmd('model_api_info'),
    retry: false,
    refetchInterval: 15_000,
  });
  const statsQ = useQuery<ModelApiStats>({
    queryKey: ['model-api-stats'],
    queryFn: () => queryCmd('model_api_stats'),
    retry: false,
    refetchInterval: (query) => (query.state.error ? false : 5_000),
  });
  const info = infoQ.data ?? null;
  const stats = statsQ.data ?? null;

  const [portDraft, setPortDraft] = useState<string | null>(null);
  const [savingPort, setSavingPort] = useState(false);
  const [toggling, setToggling] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [probing, setProbing] = useState(false);
  const [probe, setProbe] = useState<ModelApiProbe | null>(null);
  const [copied, setCopied] = useState('');
  const [models, setModels] = useState<ModelEntry[] | null>(null);
  const [loadingModels, setLoadingModels] = useState(false);
  const [modelFilter, setModelFilter] = useState('');
  const [snippetKey, setSnippetKey] = useState<'openai' | 'anthropic' | 'responses' | 'claude-code'>(
    'openai',
  );

  const refresh = () => {
    void qc.invalidateQueries({ queryKey: ['model-api-info'] });
    void qc.invalidateQueries({ queryKey: ['model-api-stats'] });
  };

  const copy = async (text: string, tag: string) => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(tag);
      window.setTimeout(() => setCopied((current) => (current === tag ? '' : current)), 1600);
    } catch {
      toast('复制失败，请手动选中复制');
    }
  };

  const toggleRun = async () => {
    if (!info) return;
    setToggling(true);
    const next = await runCmd(info.running ? 'model_api_stop' : 'model_api_start');
    setToggling(false);
    if (!next) return;
    qc.setQueryData(['model-api-info'], next);
    setPortDraft(null);
    refresh();
    if (!next.running) {
      toast('已停止监听，正在接入的客户端会连不上');
    } else if (next.port !== next.configured_port) {
      toast(`端口 ${next.configured_port} 被占用，已改用 ${next.port}`);
    } else {
      toast(`已启动：http://127.0.0.1:${next.port}/v1`);
    }
  };

  const restart = async () => {
    setRestarting(true);
    const next = await runCmd('model_api_restart');
    setRestarting(false);
    if (!next) return;
    qc.setQueryData(['model-api-info'], next);
    refresh();
    toast(
      next.port === next.configured_port
        ? `已重启：http://127.0.0.1:${next.port}/v1`
        : `已重启；配置端口 ${next.configured_port} 被占用，实际监听 ${next.port}`,
    );
  };

  const regenerate = async () => {
    const ok = await confirmAction({
      title: '重新生成 API Key？',
      message: '旧 Key 会立即作废，正在用它接入的客户端需要改用新 Key。',
    });
    if (!ok) return;
    const next = await runCmd('model_api_regenerate_key');
    if (!next) return;
    qc.setQueryData(['model-api-info'], next);
    toast('已生成新的 API Key，旧 Key 已作废');
  };

  const savePort = async () => {
    if (!info) return;
    const port = Number.parseInt((portDraft ?? '').trim(), 10);
    if (!Number.isInteger(port) || port < 1024 || port > 65535) {
      toast('端口需在 1024–65535 之间');
      return;
    }
    if (port === info.configured_port) {
      setPortDraft(null);
      return;
    }
    setSavingPort(true);
    const next = await runCmd('model_api_set_port', { port });
    setSavingPort(false);
    if (!next) return;
    qc.setQueryData(['model-api-info'], next);
    setPortDraft(null);
    refresh();
    toast(
      next.port === next.configured_port
        ? `端口已改为 ${next.port}`
        : `端口 ${next.configured_port} 被占用，已改用 ${next.port}`,
    );
  };

  const probeRun = async () => {
    setProbing(true);
    const result = await runCmd('model_api_probe');
    setProbing(false);
    if (!result) return;
    setProbe(result);
    toast(
      result.ok
        ? `自检通过：HTTP ${result.status}，${fmtMs(result.ms)}`
        : `自检未通过：${result.detail}`,
    );
  };

  const loadModels = async () => {
    setLoadingModels(true);
    const list = await runCmd('model_api_models');
    setLoadingModels(false);
    setModels(list ?? []);
  };

  const shownPort = info ? (info.running ? info.port : info.configured_port) : 0;
  const fellBack = !!info && info.running && info.port !== info.configured_port;
  const base = info ? `http://127.0.0.1:${shownPort}` : '';
  const key = info?.key ?? '';
  const portValue = portDraft ?? String(info?.configured_port ?? '');

  const visibleModels = useMemo(() => {
    const text = modelFilter.trim().toLowerCase();
    const list = models ?? [];
    return text
      ? list.filter(
          (model) => model.id.toLowerCase().includes(text) || model.name.toLowerCase().includes(text),
        )
      : list;
  }, [models, modelFilter]);

  const snippets = useMemo(() => {
    const b = base || 'http://127.0.0.1:<端口>';
    const k = key || '<API Key>';
    return {
      openai: `import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "${b}/v1",
  apiKey: "${k}",
});

const reply = await client.chat.completions.create({
  model: "<模型 ID>",
  messages: [{ role: "user", content: "你好" }],
});
console.log(reply.choices[0]?.message?.content);`,
      anthropic: `import Anthropic from "@anthropic-ai/sdk";

const client = new Anthropic({
  baseURL: "${b}",            // Anthropic SDK 自己拼 /v1/messages
  apiKey: "${k}",             // 也认 x-api-key 头
});

const msg = await client.messages.create({
  model: "<模型 ID>",
  max_tokens: 1024,
  messages: [{ role: "user", content: "你好" }],
});`,
      responses: `import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "${b}/v1",
  apiKey: "${k}",
});

const res = await client.responses.create({
  model: "<模型 ID>",
  input: "你好",
});
console.log(res.output_text);`,
      'claude-code': `# Claude Code 走环境变量，不需要改代码
ANTHROPIC_BASE_URL=${b}
ANTHROPIC_AUTH_TOKEN=${k}
ANTHROPIC_MODEL=<模型 ID>`,
    } satisfies Record<typeof snippetKey, string>;
  }, [base, key]);

  const snippetTabs: { key: typeof snippetKey; label: string; endpoint: string }[] = [
    { key: 'openai', label: 'OpenAI SDK', endpoint: 'POST /v1/chat/completions' },
    { key: 'anthropic', label: 'Anthropic SDK', endpoint: 'POST /v1/messages' },
    { key: 'responses', label: 'Responses SDK', endpoint: 'POST /v1/responses' },
    { key: 'claude-code', label: 'Claude Code', endpoint: '环境变量' },
  ];

  return (
    <div className="stagger flex flex-col gap-4">
      <Card>
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-sm font-semibold">豆包接口反代</h2>
          {info && (
            <Tag tone={info.running ? 'ok' : 'muted'}>{info.running ? '运行中' : '已停止'}</Tag>
          )}
          <span className="text-xs text-muted">只绑 127.0.0.1，不对局域网/公网暴露</span>
          <span className="flex-1" />
          {info && (
            <>
              <Button
                size="sm"
                variant={info.running ? 'ghost' : 'primary'}
                onClick={() => void toggleRun()}
                loading={toggling}
              >
                {info.running ? '停止' : '启动'}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => void restart()}
                loading={restarting}
                disabled={!info.running}
                title="停掉旧监听后按配置端口重新绑定"
              >
                重启
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => void probeRun()}
                loading={probing}
                disabled={!info.running}
                title="带 Key 真发一次 /v1/models，验证监听、鉴权与上游"
              >
                自检
              </Button>
            </>
          )}
        </div>
        <p className="mt-1 text-xs leading-relaxed text-muted">
          把豆包账号暴露成本机接口，客户端填地址、Key 和模型 ID 即可调用：
          <span className="font-mono text-fg2"> OpenAI Chat</span>、
          <span className="font-mono text-fg2"> Anthropic Messages</span>、
          <span className="font-mono text-fg2"> OpenAI Responses</span> 三种形态都支持。
        </p>

        {infoQ.isLoading && <SkeletonBlock rows={4} className="mt-3" />}
        {infoQ.isError && (
          <StateBlock
            kind="error"
            title="读取接口状态失败"
            detail={errText(infoQ.error)}
            onRetry={() => infoQ.refetch()}
          />
        )}

        {info && (
          <>
            <div className="mt-3 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
              <Metric label="运行时长" value={stats ? fmtUptime(stats.uptime_secs) : '—'} />
              <Metric label="累计请求" value={stats ? String(stats.requests) : '—'} />
              <Metric
                label="失败"
                value={stats ? String(stats.failures) : '—'}
                tone={stats && stats.failures > 0 ? 'bad' : undefined}
              />
              <Metric label="平均耗时" value={stats && stats.requests ? fmtMs(stats.avg_ms) : '—'} />
            </div>

            <div className="mt-3 flex flex-col gap-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="w-16 text-xs text-muted">地址</span>
                <code className="rounded bg-hover px-2 py-1 text-xs select-all">{base}/v1</code>
                <Button size="sm" variant="ghost" onClick={() => void copy(`${base}/v1`, 'base')}>
                  {copied === 'base' ? '已复制' : '复制地址'}
                </Button>
                {!info.running && (
                  <span className="text-xs text-muted">（未启动，客户端连不上）</span>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-2">
                <span className="w-16 text-xs text-muted">端口</span>
                <Input
                  className="w-28"
                  value={portValue}
                  onChange={(e) => setPortDraft(e.target.value)}
                  placeholder="43861"
                  inputMode="numeric"
                />
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => void savePort()}
                  loading={savingPort}
                  disabled={portDraft === null || portDraft.trim() === String(info.configured_port)}
                >
                  保存端口
                </Button>
                <span className="text-xs text-muted">
                  {info.running ? '改完立即重启监听' : '启动时按此端口监听'}
                </span>
              </div>

             {fellBack && (
                <Notice tone="warn">
                  配置端口 {info.configured_port} 当时被占用，已自动改用 {info.port}。
                  正在接入的客户端请按上面的地址填写；想固定端口就换一个没被占用的再保存。
                </Notice>
              )}

              <div className="flex flex-wrap items-center gap-2">
                <span className="w-16 text-xs text-muted">API Key</span>
                <code className="min-w-0 flex-1 truncate rounded bg-hover px-2 py-1 text-xs select-all">
                  {info.key}
                </code>
                <Button size="sm" variant="ghost" onClick={() => void copy(info.key, 'key')}>
                  {copied === 'key' ? '已复制' : '复制 Key'}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => void regenerate()}>
                  重新生成
                </Button>
              </div>
            </div>

            {probe && (
              <div className="mt-3">
                <Notice tone={probe.ok ? 'info' : 'bad'}>
                  <div className="flex flex-wrap items-center gap-2">
                    <span className={probe.ok ? 'text-ok' : 'text-bad'}>
                      自检{probe.ok ? '通过' : '未通过'}
                    </span>
                    <span className="text-xs text-muted">
                      HTTP {probe.status || '—'} · {fmtMs(probe.ms)} · {probe.url}
                    </span>
                  </div>
                  <div className="mt-1 text-xs leading-relaxed text-fg2">{probe.detail}</div>
                </Notice>
              </div>
            )}

            {stats && (
              <p className="mt-2 text-xs text-muted">
                本次监听启动于 {fmtTime(stats.started_at)}，最近一次调用{' '}
                {fmtTime(stats.last_request_at)}。停止后保留上一次监听的统计，重启归零。
              </p>
            )}
          </>
        )}
      </Card>

      {info && (
        <div className="grid gap-3 lg:grid-cols-[minmax(0,1.15fr)_minmax(0,.85fr)]">
          <Card>
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="text-sm font-semibold">接入方式</h3>
              <span className="text-xs text-muted">地址 + Key + 模型 ID，三样就够</span>
              <span className="flex-1" />
              <Button
                size="sm"
                variant="ghost"
                onClick={() => void copy(snippets[snippetKey], 'snippet')}
              >
                {copied === 'snippet' ? '已复制' : '复制示例'}
              </Button>
            </div>
            <div className="mt-2 flex flex-wrap gap-1 rounded-md border border-line p-0.5">
              {snippetTabs.map((tab) => (
                <button
                  key={tab.key}
                  type="button"
                  onClick={() => setSnippetKey(tab.key)}
                  className={cn(
                    'cursor-pointer rounded px-2 py-0.5 text-xs transition-colors',
                    snippetKey === tab.key
                      ? 'bg-accent text-on-accent'
                      : 'text-muted hover:bg-hover hover:text-fg',
                  )}
                  title={tab.endpoint}
                >
                  {tab.label}
                </button>
              ))}
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs text-muted">
              <span className="font-mono">{snippetTabs.find((tab) => tab.key === snippetKey)?.endpoint}</span>
              <Tag tone="muted">Anthropic 的 baseURL 不带 /v1</Tag>
              <Tag tone="muted">x-api-key 也认</Tag>
            </div>
            <pre className="mt-2 max-h-[22rem] overflow-auto whitespace-pre-wrap break-words rounded-md border border-line bg-bg p-2.5 font-mono text-[11px] leading-relaxed text-fg2 select-all">
              {snippets[snippetKey]}
            </pre>
          </Card>

          <Card>
            <div className="flex flex-wrap items-center gap-2">
              <h3 className="text-sm font-semibold">可用模型</h3>
              <span className="text-xs text-muted">
                {models ? `共 ${models.length} 个` : '所有启用豆包账号的并集'}
              </span>
              <span className="flex-1" />
              <Button
                size="sm"
                variant="ghost"
                onClick={() => void loadModels()}
                loading={loadingModels}
              >
                {models ? '刷新' : '获取模型'}
              </Button>
            </div>
            {models ? (
              <>
                <Input
                  className="mt-2 w-full"
                  placeholder="搜索模型名称或 ID"
                  value={modelFilter}
                  onChange={(e) => setModelFilter(e.target.value)}
                />
                {visibleModels.length === 0 ? (
                  <p className="py-6 text-center text-xs text-muted">没有匹配的模型。</p>
                ) : (
                  <div className="mt-2 flex max-h-[19rem] flex-col gap-1 overflow-y-auto rounded-md border border-line bg-bg p-2">
                    {visibleModels.map((model) => (
                      <button
                        key={model.id}
                        type="button"
                        onClick={() => void copy(model.id, `model:${model.id}`)}
                        className="flex cursor-pointer items-baseline gap-2 rounded px-2 py-1 text-left hover:bg-hover"
                        title="点击复制模型 ID"
                      >
                        <span className="shrink-0 text-xs text-fg2">{model.name}</span>
                        <code className="min-w-0 flex-1 truncate text-xs text-muted">{model.id}</code>
                        <span className="shrink-0 text-[10px] text-muted">
                          {copied === `model:${model.id}` ? '已复制' : ''}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </>
            ) : (
              <p className="mt-2 text-xs leading-relaxed text-muted">
                点「获取模型」会直接用账号凭据拉目录——接口停着也能查。客户端在 model 字段填这里列出的 ID。
              </p>
            )}
          </Card>
        </div>
      )}

      <Card>
        <div className="flex flex-wrap items-center gap-2">
          <h3 className="text-sm font-semibold">最近调用</h3>
          <span className="text-xs text-muted">
            {stats ? `最近 ${stats.recent.length} 次 · 5 秒自动刷新` : '读取统计中…'}
          </span>
          <span className="flex-1" />
          <Button size="sm" variant="ghost" onClick={() => navigate('logs', { module: 'model_api' })}>
            查看接口日志
          </Button>
        </div>
        {statsQ.isError ? (
          <StateBlock
            kind="error"
            title="读取运行统计失败"
            detail={errText(statsQ.error)}
            onRetry={() => statsQ.refetch()}
          />
        ) : !stats || stats.recent.length === 0 ? (
          <p className="py-6 text-center text-sm text-muted">
            {info?.running
              ? '还没有客户端调用。把上面的地址和 Key 填进客户端，第一次请求就会出现在这里。'
              : '接口未运行；启动后这里会记录每一次调用。'}
          </p>
        ) : (
          <table className="mt-2 w-full border-collapse text-xs">
            <thead>
              <tr className="text-left text-muted">
                <th className="w-32 py-1 font-medium">时间</th>
                <th className="py-1 font-medium">接口</th>
                <th className="w-20 py-1 font-medium">状态</th>
                <th className="w-20 py-1 text-right font-medium">耗时</th>
              </tr>
            </thead>
            <tbody>
              {stats.recent.map((call, index) => (
                <tr key={`${call.ts}-${call.path}-${index}`} className="border-t border-line/60">
                  <td className="whitespace-nowrap py-1 tabular-nums text-muted">{fmtTime(call.ts)}</td>
                  <td className="py-1">
                    <span className="text-fg2">{ENDPOINT_LABEL[call.path] ?? call.path}</span>
                    <span className="ml-1 font-mono text-[10px] text-muted">{call.path}</span>
                  </td>
                  <td className={cn('py-1 tabular-nums', call.ok ? 'text-ok' : 'text-bad')}>
                    {call.status || '—'}
                  </td>
                  <td className="py-1 text-right tabular-nums text-muted">{fmtMs(call.ms)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>

      <Notice tone="warn">
        接口只监听 127.0.0.1：本机任何程序都能访问，局域网与公网不能。Key 一旦泄露，立刻「重新生成」；
        客户端报连接失败时先点「自检」，再到日志页按模块 <span className="font-mono">model_api</span>{' '}
        看具体请求。<span className="font-mono">model_api</span> 关键字会把请求转发、换号与上游报错一起留下。
      </Notice>
    </div>
  );
}

function Metric({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: 'bad';
}) {
  return (
    <div className="rounded-md border border-line bg-bg px-3 py-2">
      <div className="text-[11px] text-muted">{label}</div>
      <div className={cn('mt-0.5 text-sm font-semibold tabular-nums', tone === 'bad' && 'text-bad')}>
        {value}
      </div>
    </div>
  );
}
