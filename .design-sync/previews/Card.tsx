// Card preview — compound composition as used across the quark panel.
import {
  Badge,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "web";

export function Composed() {
  return (
    <Card className="w-96">
      <CardHeader>
        <CardTitle>Custom domains</CardTitle>
        <CardDescription>Serve short links from your own domain.</CardDescription>
        <CardAction>
          <Badge variant="secondary">Pro</Badge>
        </CardAction>
      </CardHeader>
      <CardContent>
        <p className="text-sm text-muted-foreground">
          go.acme.dev is verified and serving 12,408 links. DNS checks run every
          15 minutes.
        </p>
      </CardContent>
      <CardFooter className="gap-2">
        <Button size="sm">Add domain</Button>
        <Button size="sm" variant="ghost">
          Manage DNS
        </Button>
      </CardFooter>
    </Card>
  );
}

export function Small() {
  return (
    <Card size="sm" className="w-72">
      <CardHeader>
        <CardTitle>Rate limits</CardTitle>
        <CardDescription>Per-IP redirect throttling.</CardDescription>
      </CardHeader>
      <CardContent>
        <p className="text-sm">120 req/min · burst 40</p>
      </CardContent>
    </Card>
  );
}
