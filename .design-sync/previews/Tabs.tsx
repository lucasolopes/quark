// Tabs preview — default (filled list) and line variants.
import { Tabs, TabsContent, TabsList, TabsTrigger } from "web";

export function Default() {
  return (
    <Tabs defaultValue="overview" className="w-96">
      <TabsList>
        <TabsTrigger value="overview">Overview</TabsTrigger>
        <TabsTrigger value="geography">Geography</TabsTrigger>
        <TabsTrigger value="devices">Devices</TabsTrigger>
      </TabsList>
      <TabsContent value="overview" className="pt-3 text-sm text-muted-foreground">
        48,215 clicks in the last 30 days across 1,982 active links.
      </TabsContent>
    </Tabs>
  );
}

export function Line() {
  return (
    <Tabs defaultValue="7d" className="w-96">
      <TabsList variant="line">
        <TabsTrigger value="24h">24h</TabsTrigger>
        <TabsTrigger value="7d">7 days</TabsTrigger>
        <TabsTrigger value="30d">30 days</TabsTrigger>
      </TabsList>
      <TabsContent value="7d" className="pt-3 text-sm text-muted-foreground">
        11,032 clicks · p99 redirect 315 ms.
      </TabsContent>
    </Tabs>
  );
}
