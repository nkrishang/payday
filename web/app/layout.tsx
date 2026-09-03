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
  metadataBase: new URL("https://payday.sh"),
  title: {
    default: "Payday — stablecoin payments that settle themselves",
    template: "%s · Payday",
  },
  description:
    "Create a USDC payment through a small API or CLI, share one link, and let Payday detect finalized transfers and settle exactly the invoice amount to your wallet automatically.",
  openGraph: {
    type: "website",
    siteName: "Payday",
    title: "Payday — stablecoin payments that settle themselves",
    description:
      "Create a USDC payment, share one link, and let Payday detect finalized transfers and settle exactly the invoice amount to your wallet automatically.",
  },
};

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#fafafa" },
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
