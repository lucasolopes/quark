// Terminal preview — mono body with quark CLI output.
import { Terminal } from "web";

export function CurlExample() {
  return (
    <div className="w-[560px]">
      <Terminal title="quark — zsh">
        {`$ curl -X POST https://quark.to/api/links \\
    -H "Authorization: Bearer qk_live_..." \\
    -d '{"url": "https://example.com/pricing"}'

{"code": "spring-sale", "short_url": "https://quark.to/spring-sale"}`}
      </Terminal>
    </div>
  );
}
