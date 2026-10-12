import type { Metadata } from "next";
import { ParalockApp } from "@/components/app/ParalockApp";
import "./app.css";

export const metadata: Metadata = {
  title: "Swap and pay",
  description:
    "Swap shielded ZEC for dollars, or pay anyone an exact amount from a payment link. Both sides settle or both refund.",
};

export default function AppPage() {
  return <ParalockApp />;
}
