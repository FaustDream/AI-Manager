// 详情面板：表格下方常驻，选中行即换。只读本地库（不打接口），动作按钮单发。
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { call, errKind, errText } from '@/lib/ipc';
import { fmtAmount, fmtDelta, fmtTime } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Dialog } from '@/components/ui/dialog';
import { Notice, Tag } from '@/components/ui';
import { toast } from '@/components/ui/toast';
import type { AccountDetail, AccountView, ClaimPlan, ClaimOutcome, ActionResult, SwitchResult } from '@/lib/types';

export function DetailPanel({ account }: { account: AccountView | null }) {
  const qc = useQueryClient();
  const [switchOpen, setSwitchOpen] = useState(false);
  const [claimPlans, setClaimPlans] = useState<ClaimPlan[] | null>(null);
  // 慢操作（签到/切换/拉活动列表都是几秒级的网络+写盘）的按钮加载态
  const [busy, setBusy] = useState<string | null>(null);
  const runBusy = (key: string, work: () => Promise<void>) => {
    if (busy) return;
    setBusy(key);
    void work().finally(() => setBusy(null));
  };

  const detail = useQuery<AccountDetail>({
    queryKey: ['detail', account?.iid],
    queryFn: () => call<AccountDetail>('checkin_detail', { iid: account!.iid }),
    enabled: !!account,
  });

  if (!account) {
    return <div className="rounded-lg border border-line bg-card p-4 text-sm text-muted">选中一行看详情。</div>;
  }

  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['accounts'] });
    qc.invalidateQueries({ queryKey: ['detail', account.iid] });
  };

  const runOne = () =>
    runBusy('checkin', async () => {
      try {
        const r = await call<ActionResult>('checkin_run', { iid: account.iid });
        toast(r.message);
        refresh();
      } catch (e) {
        toast(errText(e));
      }
    });

  const doSwitch = () =>
    runBusy('switch', async () => {
      setSwitchOpen(false);
      try {
        const r = await call<SwitchResult>('account_switch', { iid: account.iid });
        toast(r.message + (r.backup ? `\n备份：${r.backup}` : ''));
        qc.invalidateQueries({ queryKey: ['current'] });
      } catch (e) {
        // 切换失败代价高，kind 给界面机会说清下一步（如 client_running 要先关客户端）
        const kind = errKind(e);
        toast(kind === 'client_running' ? `请先关闭客户端再切换。\n${errText(e)}` : errText(e));
      }
    });

  const openClaim = () =>
    runBusy('plans', async () => {
      try {
        const plans = await call<ClaimPlan[]>('zcode_claim_plans', { iid: account.iid });
        setClaimPlans(plans);
      } catch (e) {
        toast(errText(e));
      }
    });

  const claim = (planId: string, name: string) =>
    runBusy('claim', async () => {
      setClaimPlans(null);
      toast(`已打开浏览器，请在页面里完成验证…`);
      try {
        const r = await call<ClaimOutcome>('zcode_claim_run', { iid: account.iid, planId });
        toast(`领取成功：${r.name || name}`);
        refresh();
      } catch (e) {
        toast(errText(e));
      }
    });

  const d = detail.data;
  return (
    <div className="rounded-lg border border-line bg-card p-4">
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <h2 className="text-sm font-semibold">{account.name}</h2>
        <Tag>{account.platform_label}</Tag>
        {account.requires_checkin && (
          <Tag tone={account.checked_in === true ? 'ok' : 'muted'}>
            {account.checked_in === true ? '今日已签' : '今日未签'}
          </Tag>
        )}
        <span className="flex-1" />
        {account.platform === 'zcode' && (
          <Button size="sm" onClick={openClaim} disabled={!!busy}>
            {busy === 'plans' ? '查询中…' : '领取活动'}
          </Button>
        )}
        {account.requires_checkin && (
          <Button size="sm" variant="primary" onClick={runOne} disabled={!!busy}>
            {busy === 'checkin' ? '签到中…' : '签到'}
          </Button>
        )}
        <Button
          size="sm"
          onClick={() => setSwitchOpen(true)}
          disabled={!account.has_secret || !!busy}
        >
          切换到此账号
        </Button>
        <Button
          size="sm"
          variant="danger"
          disabled={!!busy}
          onClick={async () => {
            // 不可逆动作（同页的会话删除、清空回收站都有确认）
            if (!window.confirm(`从账号库移除「${account.name}」？`)) return;
            try {
              await call('account_delete', { iid: account.iid });
              toast('已从账号库移除（客户端登录态未动）');
              refresh();
            } catch (e) {
              toast(errText(e));
            }
          }}
        >
          删除
        </Button>
      </div>
      <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
        <dt className="text-xs text-muted">余额</dt>
        <dd className="tabular-nums">{fmtAmount(d?.balance ?? account.balance)}</dd>
        <dt className="text-xs text-muted">今日增减</dt>
        <dd className={`tabular-nums ${(d?.today_delta ?? 0) > 0 ? 'text-ok' : ''}`}>
          {fmtDelta(d?.today_delta)}
        </dd>
        <dt className="text-xs text-muted">下次可签</dt>
        <dd>{d?.next_at ? fmtTime(d.next_at) : '—'}</dd>
        <dt className="text-xs text-muted">最近批次</dt>
        <dd className="flex flex-wrap gap-1">
          {(d?.lots ?? []).slice(0, 5).map((lot) => (
            <Tag key={lot.lot_key} tone="muted">
              {fmtAmount(lot.amount)}
              {lot.expires_at ? ` · ${fmtTime(lot.expires_at)} 到期` : ''}
            </Tag>
          ))}
          {(d?.lots ?? []).length === 0 && <span className="text-muted">—</span>}
        </dd>
      </dl>
      {account.error && (
        <Notice tone="bad" className="mt-2">
          最近一次查询失败：{account.error}
        </Notice>
      )}

      <Dialog
        open={switchOpen}
        title="切换登录"
        onClose={() => setSwitchOpen(false)}
        footer={
          <>
            <Button onClick={() => setSwitchOpen(false)}>取消</Button>
            <Button variant="primary" onClick={doSwitch}>
              确认切换
            </Button>
          </>
        }
      >
        <p className="whitespace-pre-wrap text-sm text-muted">
          会写客户端登录态文件并重启客户端；失败需要重新登录。
          {`当前账号：${account.name}`}
        </p>
      </Dialog>

      <Dialog
        open={claimPlans !== null}
        title="可领取的活动"
        onClose={() => setClaimPlans(null)}
        wide
      >
        {(claimPlans ?? []).length === 0 ? (
          <p className="text-sm text-muted">当前没有可领取的活动。</p>
        ) : (
          <div className="flex flex-col gap-2">
            {(claimPlans ?? []).map((plan) => (
              <div key={plan.plan_id} className="rounded-md border border-line p-2 text-sm">
                <div className="flex items-center gap-2">
                  <span className="font-semibold">{plan.name || plan.plan_id}</span>
                  {plan.campaign && <Tag tone="warn">{plan.campaign.title}</Tag>}
                  <span className="flex-1" />
                  <Button size="sm" variant="primary" onClick={() => claim(plan.plan_id, plan.name)}>
                    领取
                  </Button>
                </div>
                {plan.description && <div className="text-xs text-muted">{plan.description}</div>}
                {plan.grants.length > 0 && (
                  <div className="mt-1 text-xs">{plan.grants.join('；')}</div>
                )}
              </div>
            ))}
          </div>
        )}
      </Dialog>
    </div>
  );
}
