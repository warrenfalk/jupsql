import type { ConnectionInput } from './model';

export function initialConnection(): ConnectionInput {
  return {
    host: 'localhost',
    port: 5432,
    database: 'postgres',
    user: 'postgres',
    password: 'postgres',
    tls: false,
  };
}
