# Fixtures updater

Les tests Rust génèrent une paire Ed25519 déterministe réservée aux fixtures, signent « fixture update » et vérifient le manifeste, les limites, le downgrade, la plateforme, la rotation, la récupération absente et l'altération du payload. Cette clé n'est jamais incluse dans une build de production.
