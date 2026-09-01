import type { Metadata } from "next";
import { LoginForm } from "@/components/dashboard/login-form";

export const metadata: Metadata = { title: "Sign in" };

export default function LoginPage() {
  return <LoginForm />;
}
