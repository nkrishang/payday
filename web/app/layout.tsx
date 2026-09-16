import type { Metadata, Viewport } from "next";
import { Albert_Sans, Inter, JetBrains_Mono } from "next/font/google";
import "./globals.css";

const inter = Inter({
  subsets: ["latin"],
  display: "swap",
  variable: "--font-inter",
});

const albertSans = Albert_Sans({
  subsets: ["latin"],
  display: "swap",
  variable: "--font-albert",
});

const jetbrainsMono = JetBrains_Mono({
  subsets: ["latin"],
  display: "swap",
  variable: "--font-mono-face",
});

export const metadata: Metadata = {
  metadataBase: new URL("https://gum.money"),
  title: {
    default: "Gum — accept stablecoin deposits into your app.",
    template: "%s · Gum",
  },
  description:
    "Create a unique programmable address for every deposit. Control who can fund it, when it expires and where it settles.",
  openGraph: {
    type: "website",
    siteName: "Gum",
    title: "Gum — accept stablecoin deposits into your app.",
    description:
      "Create a unique programmable address for every deposit. Control who can fund it, when it expires and where it settles.",
    url: "https://gum.money",
  },
  twitter: {
    // Large card on X, which reads these tags ahead of the OpenGraph ones.
    card: "summary_large_image",
    title: "Gum — accept stablecoin deposits into your app.",
    description:
      "Create a unique programmable address for every deposit. Control who can fund it, when it expires and where it settles.",
  },
};

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#f7f7f5" },
    { media: "(prefers-color-scheme: dark)", color: "#0b0b0c" },
  ],
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html
      lang="en"
      className={`${inter.variable} ${albertSans.variable} ${jetbrainsMono.variable}`}
    >
      <body className="font-sans antialiased">{children}</body>
    </html>
  );
}
