export function plain(text: string): string {
  return text.replace(/[⁨⁩]/g, "");
}
