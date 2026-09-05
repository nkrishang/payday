import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";

/**
 * "Add one of these." Every table on the dashboard gets the same control in
 * the same place, because they are the same kind of act: a deposit request, an
 * issuer identity, and a customer are all one button away, and none of them
 * should look more important than another by accident.
 *
 * `label` names what is being added for assistive tech; the button itself
 * reads "New", which the section around it already qualifies.
 */
export function AddButton({
  label,
  onClick,
  className,
}: {
  label: string;
  onClick: () => void;
  className?: string;
}) {
  return (
    <Button
      type="button"
      variant="secondary"
      size="sm"
      aria-label={label}
      onClick={onClick}
      className={cn("h-9 gap-1.5 px-3", className)}
    >
      <Plus className="size-3.5" />
      New
    </Button>
  );
}
