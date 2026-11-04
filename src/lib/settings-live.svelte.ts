/// 设置页保存后，正在显示的界面要能立刻按新设置重画（现在只有导航栏的动画时长用到）。
/// 不把整个 Settings 搬过来，只发一个「变过了」的信号，需要的人自己去读。
export const settingsLive = $state({ version: 0 });

export function notifySettingsSaved() {
  settingsLive.version += 1;
}
