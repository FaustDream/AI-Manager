// barrel：页面统一从 @/components/ui 拿表面组件。
// 分层职责见 docs：card 管版式容器，badge 管状态标签，form 管输入与占位态。
export { Card, Section } from './card';
export { Notice, Tag } from './badge';
export { Button, cn } from './button';
export { ConfirmHost, confirmAction, useConfirm } from './confirm';
export { Dialog } from './dialog';
export { Drawer } from './drawer';
export {
  Checkbox,
  Field,
  Input,
  Select,
  SkeletonBlock,
  SkeletonRows,
  StateBlock,
  Textarea,
} from './form';
export { LoadMore } from './table';
export { toast, ToastHost, useToasts } from './toast';