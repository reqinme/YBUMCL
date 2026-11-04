/// 下载页里的「下载任务」是一层页内覆盖层，不再单独开路由——
/// 这样进来再返回时，下载页的分类、搜索条件、详情、滚动位置都还在（和 PCL 一样）。
export const downloadsNav = $state<{ tasksOpen: boolean }>({ tasksOpen: false });
