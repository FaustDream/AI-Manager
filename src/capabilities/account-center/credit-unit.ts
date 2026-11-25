/**
 * 余额单位：ima 计的是「算力」，其余平台沿用「积分」。
 * 只用于账户中心的余额/签到收益展示，不改变平台接口字段名。
 */
export const creditUnit = (platform: string): string => (platform === 'ima' ? '算力' : '积分');
