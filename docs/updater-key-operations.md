# Exploitation des clés de mise à jour

Les builds de développement n'embarquent aucune clé et affichent « mises à jour indisponibles ». Une build stable doit définir DEVTOOLBOX_RELEASE_BUILD=1 et DEVTOOLBOX_UPDATE_PUBLIC_KEYS vers un JSON associant chaque key_id à sa clé Ed25519 en base64. build.rs refuse une release stable sans trousseau valide. Les clés privées ne résident jamais dans le dépôt ni dans les artefacts.

Le client accepte une signature connue dont la date d'expiration n'est pas dépassée et dont l'activation remonte au plus à deux versions mineures. Une rotation publie les signatures de l'ancienne et de la nouvelle clé pendant deux versions mineures et au moins 180 jours. Après cette fenêtre, un client trop ancien effectue une réinstallation manuelle signée par l'OS.

En cas de compromission :

1. retirer la clé du trousseau protégé et révoquer les certificats OS concernés ;
2. publier un avis et une release corrigée signée par une clé hors ligne saine ;
3. ne jamais demander au client d'ignorer une erreur de signature ;
4. imposer la réinstallation manuelle aux clients qui ne connaissent aucune clé saine.

Les empreintes SHA-256 des clés publiques embarquées sont produites par KeyRing::fingerprints et doivent être jointes au manifeste de build. Les payloads, y compris celui de récupération de la version courante, restent sur une URL de tag GitHub immuable.
