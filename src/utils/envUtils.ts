export function envVarToBoolean(envVar: string | undefined): boolean {
  if (!envVar) {
    return false;
  }
  return envVar.toLowerCase() === 'true';
}
