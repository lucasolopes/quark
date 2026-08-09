// Toaster preview — fires real toasts on mount so the open state renders.
import { useEffect } from "react";
import { Toaster, toast } from "web";

export function Toasts() {
  useEffect(() => {
    toast.success("Link created", { description: "quark.to/spring-sale is live." });
    toast.error("Domain verification failed", { description: "DNS record not found for go.acme.dev." });
  }, []);
  return (
    <div className="relative h-64 w-[420px]">
      <Toaster position="bottom-right" expand />
    </div>
  );
}
