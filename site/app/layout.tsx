import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";

const geistSans = Geist({ variable: "--font-geist-sans", subsets: ["latin"] });
const geistMono = Geist_Mono({ variable: "--font-geist-mono", subsets: ["latin"] });

export const metadata: Metadata = {
  metadataBase: new URL("https://sagascript.gille.ai"),
  title: "Sagascript — Local dictation for Mac",
  description: "Fast, private dictation and transcription for Apple silicon Macs. Speech is processed locally on your computer.",
  icons: { icon: "/favicon.svg" },
  openGraph: {
    title: "Sagascript — Local dictation for Mac",
    description: "Speak. Get text. Keep it local.",
    images: [{ url: "/og.png", width: 1200, height: 630, alt: "Sagascript — Speak. Get text. Keep it local." }],
  },
  twitter: {
    card: "summary_large_image",
    title: "Sagascript — Local dictation for Mac",
    description: "Speak. Get text. Keep it local.",
    images: ["/og.png"],
  },
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body className={`${geistSans.variable} ${geistMono.variable}`}>{children}</body>
    </html>
  );
}
