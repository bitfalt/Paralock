import { Developers } from "@/components/Developers";
import { ParalockApp } from "@/components/ParalockApp";
import { PrivacyLevels } from "@/components/PrivacyLevels";
import { SiteFooter } from "@/components/SiteFooter";

export default function Home() {
  return (
    <>
      <ParalockApp>
        <PrivacyLevels />
        <Developers />
      </ParalockApp>
      <SiteFooter />
    </>
  );
}
