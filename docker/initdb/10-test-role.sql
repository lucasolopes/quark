-- Role e banco dedicados aos testes gated (QUARK_TEST_DATABASE_URL).
--
-- O POSTGRES_USER do compose (quark) e superusuario, e o Postgres isenta
-- superusuario de Row Level Security a menos que a role tenha NOBYPASSRLS.
-- Rodar a suite com ele mascara o FORCE ROW LEVEL SECURITY do modo cloud, que
-- e o mecanismo de isolamento entre tenants: os testes passariam mesmo com o
-- RLS quebrado. Esta role espelha producao (sem superusuario) e e dona do
-- proprio banco, o que ja da CREATE no schema public a partir do PG15.
--
-- Rodado uma unica vez, na primeira inicializacao do volume do Postgres.
CREATE ROLE quark_test LOGIN PASSWORD 'quark_test' NOSUPERUSER NOBYPASSRLS NOCREATEROLE;
CREATE DATABASE quark_test OWNER quark_test;
