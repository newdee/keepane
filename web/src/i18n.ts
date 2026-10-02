// The page speaks the phone's language when it is Chinese, else English.
export const zh = /^zh/i.test(navigator.language);
export const t = (en: string, cn: string) => (zh ? cn : en);
