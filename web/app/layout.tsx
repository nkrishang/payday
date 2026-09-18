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
    default: "Gum — stablecoin deposits that stick.",
    template: "Gum · %s",
  },
  description:
    "Create a unique programmable address for every deposit. Control who can fund it and where it settles. Let your users pay from any source.",
  openGraph: {
    type: "website",
    siteName: "Gum",
    title: "Gum — stablecoin deposits that stick.",
    description:
      "Create a unique programmable address for every deposit. Control who can fund it and where it settles. Let your users pay from any source.",
    url: "https://gum.money",
  },
  twitter: {
    // Large card on X, which reads these tags ahead of the OpenGraph ones.
    card: "summary_large_image",
    site: "@gum_money",
    title: "Gum — stablecoin deposits that stick.",
    description:
      "Create a unique programmable address for every deposit. Control who can fund it and where it settles. Let your users pay from any source.",
  },
};

export const viewport: Viewport = {
  // Every page a visitor or merchant lands on is Gum's white, on either
  // colour scheme; only the docs keep a dark ground.
  themeColor: "#f7f7f5",
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
