import checkSession from '../assets/menu/check-session.png';
import browser from '../assets/menu/browser-profile.png';
import mail from '../assets/menu/mail.png';
import rename from '../assets/menu/rename.png';
import checkConnection from '../assets/menu/check-connection.png';
import archive from '../assets/menu/archive.png';
import manage from '../assets/menu/manage-account.png';
import restore from '../assets/menu/restore.png';
import connect from '../assets/menu/connect.png';
import switchAccount from '../assets/menu/switch-account.png';
import remove from '../assets/menu/delete.png';
import renew from '../assets/menu/renew.png';

export const menuAssets = {
  checkSession,
  browser,
  mail,
  rename,
  checkConnection,
  archive,
  manage,
  restore,
  connect,
  switchAccount,
  remove,
  renew,
} as const;
export type MenuGlyph = keyof typeof menuAssets;
