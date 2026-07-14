[English](REDIRECT-RULES.md) · **Português**

# Regras de redirecionamento (geo/dispositivo)

Roadmap #12. Um único link curto pode resolver para destinos diferentes
dependendo de quem está clicando: o país ou o tipo de dispositivo. É um
motor de regras no estilo Shlink, construído em cima dos dados que o
quark já lê para o analytics de cliques.

## O modelo

Cada link tem uma lista ordenada de regras, além da sua `url` normal:

- **As regras são avaliadas em ordem.** A primeira regra cuja condição
  combina com o visitante vence.
- **`url` é o destino padrão.** Se nenhuma regra combinar (ou o link não
  tiver nenhuma regra), o visitante vai para `url`, exatamente como antes
  dessa funcionalidade existir.
- Uma regra nunca muda o código do link, o alias ou o analytics. Ela só
  muda para qual destino um clique é resolvido.

Uma regra tem três partes:

```json
{ "field": "country", "values": ["BR", "PT"], "to": "https://exemplo.com/lp-pt" }
```

- `field`: o que combinar. `"country"` ou `"device"`.
- `values`: a lista de valores que fazem essa regra combinar. Uma regra
  combina quando o valor do visitante está nessa lista.
- `to`: o destino do redirect quando essa regra combina. Validado do
  mesmo jeito que a `url` principal (precisa ser `http://` ou `https://`,
  e não pode apontar para um host interno/bloqueado).

## Campos

### `country`

Comparado com o código ISO de duas letras do país do visitante, por
exemplo `BR`, `US`, `PT`. A API deixa em maiúsculas o que você enviar,
então `br` e `BR` são equivalentes.

**Isso exige que o edge na frente do quark envie um header
`cf-ipcountry`** (o Cloudflare faz isso automaticamente em qualquer
requisição que passa por ele). Sem esse header, o quark não tem como
saber o país do visitante, e as regras de país simplesmente nunca
combinam, caindo para a `url` padrão. Veja `docs/EDGE.md` pro setup de
edge atual.

### `device`

Comparado com uma categoria grosseira de dispositivo, derivada do
`User-Agent` do visitante: `Mobile`, `Desktop` ou `Other`. A API
normaliza a caixa, então `mobile` vira `Mobile`. Regras de nível de SO ou
navegador (ex.: "só iOS") estão fora de escopo por enquanto; precisam de
um parser de User-Agent mais fino que ainda não existe no quark.

## Limites

- Até 20 regras por link.
- As regras são opcionais. A maioria dos links não tem nenhuma, e não
  paga nada a mais no redirect: o teste "esse link tem regras" é um único
  `is_empty()`, mesmo custo dos dois jeitos.

## Exemplo

Um link com `url: "https://exemplo.com"` e duas regras:

```json
[
  { "field": "country", "values": ["BR"], "to": "https://exemplo.com/br" },
  { "field": "device", "values": ["Mobile"], "to": "https://exemplo.com/m" }
]
```

- Visitante do Brasil, em qualquer dispositivo: vai para
  `https://exemplo.com/br` (a regra de país é a primeira e combina).
- Visitante dos EUA, no celular: não combina com país, cai na regra de
  dispositivo, vai para `https://exemplo.com/m`.
- Visitante dos EUA, no desktop: nenhuma regra combina, vai para o padrão
  `https://exemplo.com`.

## Gerenciando regras

No painel web, os diálogos de criar e editar link têm uma seção
colapsável "Regras de redirecionamento". Adicione uma linha por regra,
escolha o campo, digite os valores (separados por vírgula, ex.: `BR,
PT`) e defina o destino. O campo de URL do próprio link continua sendo o
destino padrão e não é afetado pela seção de regras.

## API

`POST /` (criar) e `PATCH /admin/links/:code` aceitam um array `rules`
opcional no corpo da requisição, no formato descrito acima.
`GET /admin/links` retorna as `rules` atuais de cada link.
