import { CopyButton } from "@/components/ui/copy-button";

export const INSTALL_COMMAND =
  "curl --fail --proto '=https' --tlsv1.2 https://raw.githubusercontent.com/nkrishang/payday/main/scripts/install.sh | sh";

export function InstallLine() {
  return (
    <div className="flex items-center gap-2 rounded-[10px] border border-line bg-surface py-1.5 pr-1.5 pl-3.5">
      <code className="min-w-0 flex-1 overflow-x-auto font-mono text-[12.5px] whitespace-nowrap text-muted">
        <span className="text-faint select-none">$ </span>
        curl …/install.sh | sh
      </code>
      <CopyButton value={INSTALL_COMMAND} label="install command" className="bg-raised" />
    </div>
  );
}
