import * as React from "react"
import { Dialog as DialogPrimitive } from "@base-ui/react/dialog"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { XIcon } from "lucide-react"

function Dialog({ ...props }: DialogPrimitive.Root.Props) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogTrigger({ ...props }: DialogPrimitive.Trigger.Props) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({ ...props }: DialogPrimitive.Portal.Props) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({ ...props }: DialogPrimitive.Close.Props) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

/**
 * Whether the nearest `DialogContent` ancestor is running in
 * `fullScreenOnMobile` mode. `DialogFooter` reads this to decide whether to
 * become a sticky, edge-to-edge action bar below `sm` — threading it as a
 * prop instead would mean touching every call site that renders a
 * `DialogFooter` (CreateLinkDialog, EditLinkDialog, LinkQrDialog, ...) just
 * to forward a value already known one level up. Defaults to `false`, so a
 * `DialogFooter` inside a `DialogContent` that never sets the prop (every
 * small dialog: confirms, QR, tokens, invites, ...) behaves exactly as
 * before.
 */
const DialogFullScreenMobileContext = React.createContext(false)

function DialogOverlay({
  className,
  ...props
}: DialogPrimitive.Backdrop.Props) {
  return (
    <DialogPrimitive.Backdrop
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-0 isolate z-50 bg-black/60 backdrop-blur-[4px] duration-100 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className
      )}
      {...props}
    />
  )
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  fullScreenOnMobile = false,
  ...props
}: DialogPrimitive.Popup.Props & {
  showCloseButton?: boolean
  /**
   * Below `sm`, render as a full-viewport sheet instead of the centered
   * card: edge-to-edge, no corner radius, scrollable body, and a sticky
   * `DialogFooter` (see the context above) so its actions stay reachable
   * without scrolling past the form. For large forms (Create/Edit link)
   * where the centered card leaves cramped margins on a phone; small
   * dialogs (confirms, QR, tokens, invites, ...) leave this off and keep
   * today's centered look untouched.
   */
  fullScreenOnMobile?: boolean
}) {
  return (
    <DialogFullScreenMobileContext.Provider value={fullScreenOnMobile}>
      <DialogPortal>
        <DialogOverlay />
        <DialogPrimitive.Popup
          data-slot="dialog-content"
          className={cn(
            "fixed top-1/2 left-1/2 z-50 grid w-full max-w-[540px] -translate-x-1/2 -translate-y-1/2 gap-4 rounded-[16px] border border-input bg-card p-6 text-sm text-popover-foreground shadow-modal data-open:animate-rise data-closed:animate-rise-out outline-none",
            // The `-translate-x/y-1/2` above compile to the CSS `translate`
            // property (Tailwind v4's dedicated property for translate/
            // scale/rotate utilities), never `transform` — so they don't
            // fight `animate-rise`/`animate-rise-out`, which animate the
            // separate `transform` property directly in their keyframes
            // (`translateY(14px)` <-> `none`). The two compose instead of
            // colliding: final position = centering translate + animation
            // offset. That's also why full-screen mode below only zeroes
            // `translate` (`max-sm:translate-x/y-0`) and never touches the
            // animation classes — the rise/rise-out motion keeps working
            // unmodified on top of it, just anchored to the viewport edge
            // instead of the viewport center.
            fullScreenOnMobile &&
              "max-sm:inset-0 max-sm:h-dvh max-sm:max-w-none max-sm:translate-x-0 max-sm:translate-y-0 max-sm:rounded-none",
            className
          )}
          {...props}
        >
          {children}
          {showCloseButton && (
            <DialogPrimitive.Close
              data-slot="dialog-close"
              render={
                <Button
                  variant="ghost"
                  className="absolute top-2 right-2"
                  size="icon-sm"
                />
              }
            >
              <XIcon
              />
              <span className="sr-only">Close</span>
            </DialogPrimitive.Close>
          )}
        </DialogPrimitive.Popup>
      </DialogPortal>
    </DialogFullScreenMobileContext.Provider>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  const fullScreenOnMobile = React.useContext(DialogFullScreenMobileContext)
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "-mx-6 -mb-6 flex flex-col-reverse gap-2 rounded-b-[16px] border-t bg-muted/50 p-6 sm:flex-row sm:justify-end",
        // In full-screen mode the footer is pinned by the flex column (the
        // body scrolls via overflow-y-auto); these overrides only restore
        // opacity and square corners at the sheet's edge. Never apply sticky
        // for the small, centered dialogs, which must keep today's footer
        // untouched. `-mx-6 -mb-6` still exactly cancels the `p-6` on
        // `DialogContent` at every size, so the footer stays flush with the
        // sheet's edges without bleeding past them, full-screen or not.
        fullScreenOnMobile && "max-sm:rounded-b-none max-sm:bg-card",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close render={<Button variant="outline" />}>
          Close
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({ className, ...props }: DialogPrimitive.Title.Props) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn(
        "font-heading text-xl font-bold tracking-[-0.02em]",
        className
      )}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: DialogPrimitive.Description.Props) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "text-sm text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
