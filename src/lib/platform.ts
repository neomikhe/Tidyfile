export type Platform = "windows" | "macos" | "linux";

export function detectPlatform(userAgent: string): Platform {
  if (/Windows/i.test(userAgent)) {
    return "windows";
  }
  return /Macintosh|Mac OS X/i.test(userAgent) ? "macos" : "linux";
}
