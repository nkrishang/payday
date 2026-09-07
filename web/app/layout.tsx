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
    default: "Payday — make every stablecoin accountable.",
    template: "%s · Payday",
  },
  description:
    "Turn stablecoin transfers into verified customer deposits, ready for your app to credit.",
  openGraph: {
    type: "website",
    siteName: "Payday",
    title: "Payday — make every stablecoin accountable.",
    description:
      "Turn stablecoin transfers into verified customer deposits, ready for your app to credit.",
    url: "https://payday.sh",
  },
  twitter: {
    // Large card on X, which reads these tags ahead of the OpenGraph ones.
    card: "summary_large_image",
    title: "Payday — make every stablecoin accountable.",
    description:
      "Turn stablecoin transfers into verified customer deposits, ready for your app to credit.",
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
