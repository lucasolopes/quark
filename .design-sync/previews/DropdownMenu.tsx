// DropdownMenu preview — open row-actions menu as on the links table.
import { MoreHorizontal } from "lucide-react";
import {
  Button,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "web";

export function RowActions() {
  return (
    <div className="flex h-72 w-80 items-start justify-start pt-2">
      <DropdownMenu open>
        <DropdownMenuTrigger
          render={<Button variant="outline" size="icon" aria-label="Link actions" />}
        >
          <MoreHorizontal />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuGroup>
            <DropdownMenuLabel>quark.to/spring-sale</DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem>Copy short URL</DropdownMenuItem>
            <DropdownMenuItem>View analytics</DropdownMenuItem>
            <DropdownMenuItem>Edit destination</DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive">Delete link</DropdownMenuItem>
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
