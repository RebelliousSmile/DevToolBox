# Orchestrateur de modèles locaux

Ce module stdlib Python alimente l’onglet **Modèles** de DevToolBox sous Windows et
Linux. Il inventorie Ollama, Jan, LM Studio et ComfyUI, télécharge vers une
bibliothèque neutre, puis aide à partager un même artefact vérifié sans promettre
qu’une simple ressemblance de nom correspond aux mêmes octets.

## Prérequis et surfaces prises en charge

- Python 3, lancé par DevToolBox via `DEVTOOLBOX_PYTHON`, le venv local, `python3`
  ou `python` ; les pipes sont forcés en UTF-8 et sans console Windows.
- Ollama : API HTTP strictement loopback et store `manifests/blobs` reconnu.
- Hugging Face : CLI `hf`; Xet est utilisé par le CLI lorsqu’il est disponible.
- LM Studio : CLI `lms` activé et sortie structurée prise en charge.
- Jan et ComfyUI : inventaire automatique, intégration guidée lorsqu’aucune API
  publique non interactive ne permet une mutation fiable.

Les emplacements détectés incluent les réglages/variables documentés avant les
valeurs par défaut : `OLLAMA_MODELS`, `LM_STUDIO_MODELS_DIR`, `JAN_DATA_FOLDER`,
`COMFYUI_MODELS_DIR` et `COMFYUI_REGISTERED_MODEL_ROOTS`. Les replis principaux
sont `%USERPROFILE%\.ollama\models` / `/usr/share/ollama/.ollama/models`,
`~/.lmstudio/models`, `%APPDATA%\Jan\data` / `~/.local/share/Jan/data`, et
`%APPDATA%\ComfyUI\models` / `~/ComfyUI/models`. Chaque racine conserve sa source
et son niveau de confiance dans le catalogue.

La bibliothèque DevToolBox vaut par défaut
`%LOCALAPPDATA%\DevToolBox\models` sous Windows et
`${XDG_DATA_HOME:-~/.local/share}/devtoolbox/models` sous Linux. Changer ce réglage
ne déplace jamais les modèles déjà présents.

## Identifiants exacts

```text
ollama://namespace/model:tag
hf://organisation/depot@<commit-40-hex>/chemin/model.gguf
lmstudio://publisher/model/fichier.gguf
https://hote/chemin/model.gguf
```

Une URL directe doit être HTTPS (HTTP loopback seulement), sans redirection vers
une origine privée et avec un SHA-256 fourni quand l’origine n’apporte pas une
identité immuable. Les formats reconnus sont validés de façon bornée : GGUF pour
les LLM, SafeTensors et checkpoints associés pour les catégories image. Aucune
conversion implicite, installation de dépendance ou réécriture de YAML/métadonnée
utilisateur n’est effectuée.

## Commandes utiles

Depuis la racine du dépôt :

```bash
python3 -m scripts.model_orchestrator inventory
python3 -m scripts.model_orchestrator providers
python3 -m scripts.model_orchestrator resolve \
  'hf://org/repo@0123456789abcdef0123456789abcdef01234567/model.gguf' \
  --family llm
python3 -m scripts.model_orchestrator recovery
python3 -m scripts.model_orchestrator settings
```

`resolve` renvoie un `review_digest`. L’UI le transmet à `download
--review-digest`; l’exécution re-résout l’offre et refuse `reviewed-offer-stale`
si le plan exact diffère de celui affiché. Les requêtes signées sont expurgées des
sorties et ne sont pas persistées.

## Bibliothèque, coûts et classement

Les écritures passent par un staging possédé, un journal fsync, une validation
bornée puis un commit atomique. La vue distingue taille logique, allocation
physique, copie, lien physique et allocation partagée. Un lien n’est proposé que
si l’identité et les volumes le permettent ; sinon une copie explicite conserve
un coût disque honnête.

L’historique local conserve les dix dernières observations terminales par
fournisseur/type. Un cache complet vérifié gagne. Après trois succès pertinents :

```text
prédit = médiane(démarrage)
       + réseau_restant / débit_réseau_médian
       + copie_locale / débit_copie_médian
ajusté = prédit / max(taux_de_succès, 0,25)
```

Une estimation insuffisamment échantillonnée reste « inconnue » et suit l’ordre à
froid configurable (Ollama, Hugging Face, LM Studio, direct par défaut). Le choix
manuel gagne toujours. Un fallback automatique ne peut jamais changer digest,
révision, fichier, format ou variante.

## Migration, reprise et retrait

- Ollama et LM Studio utilisent uniquement leurs surfaces natives documentées,
  avec identité/source/destination revalidées avant chaque étape.
- Jan indique l’action manuelle exacte et reprend après observation du modèle.
- ComfyUI utilise une configuration DevToolBox séparée quand le hook de lancement
  est pris en charge ; `extra_model_paths.yaml` de l’utilisateur reste intact.
- Un journal interrompu ne propose `resume`, `rollback` ou `discard-partial` que
  si la capacité et le chemin possédé sont de nouveau prouvés.
- En v1, seul le propriétaire Ollama peut être retiré. Il faut une identité
  vérifiée, une validation forte de destination et aucune référence loaded,
  workflow, keep, provisoire, en cours ou tierce. Un jeton court lié à l’état est
  revérifié juste avant l’API DELETE loopback. Le résultat distingue octets
  logiques, évités, estimés récupérables et réellement mesurés après inventaire.

LM Studio, Jan, ComfyUI et les stores inconnus restent report-only pour la
suppression. Une réussite de migration ne crée jamais à elle seule une autorité
de suppression.

## Tests et livraison

```bash
python3 -m unittest discover -s scripts/model_orchestrator/tests
cargo fmt --check
cargo check
cargo clippy -- -D warnings
cargo test
cargo build --release
```

Linux exécute les fixtures Python/Rust et les scénarios d’annulation localement.
La livraison Windows doit en plus vérifier `%LOCALAPPDATA%`, les volumes
personnalisés et points de jonction/reparse, les CLI natifs, les réglages Jan/LM,
les pipes UTF-8 sans fenêtre console et l’arrêt des descendants sur annulation.
