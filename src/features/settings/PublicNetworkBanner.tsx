import type { Settings } from "../../api";
import { useNetwork } from "../../data";
import { Icon } from "../../ui/Icon";
import { t } from "../../i18n";

/** Under the title bar while the public-network guard is on and a Public network is connected
 * (`network`): what LRAM refuses there. */
export function PublicNetworkBanner({ settings }: { settings: Settings }) {
  const onPublic = useNetwork().data?.public;
  if (!onPublic || settings.allowPublicNetworks) return null;
  return (
    <div role="status" className="enter panel flex-none flex flex-wrap items-center gap-x-4 gap-y-2 mx-3 mt-1 mb-2 px-4 py-2.5">
      <Icon name="lock" />
      <span className="flex-1 min-w-0">{t("settings.publicBannerBlocked")}</span>
    </div>
  );
}
