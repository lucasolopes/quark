// Dialog preview — open state rendered statically inside the card.
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
} from "web";

export function Open() {
  return (
    <div className="h-[420px] w-[560px]">
      <Dialog open>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Edit link</DialogTitle>
            <DialogDescription>Change the destination without breaking the short URL.</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-2 py-2">
            <Label htmlFor="edit-dest">Destination URL</Label>
            <Input id="edit-dest" defaultValue="https://example.com/pricing" />
          </div>
          <DialogFooter>
            <Button variant="ghost">Cancel</Button>
            <Button>Save changes</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
