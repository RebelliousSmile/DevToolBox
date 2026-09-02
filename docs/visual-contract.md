# Contrat visuel DevToolBox 0.10

La source exécutable des tokens est `src/ui/theme.rs`. Toute nouvelle vue utilise la grille de 4 px, des espacements multiples de 8 px, des cartes de rayon 12 px et des contrôles de rayon 8 px.

## Couleur et typographie

Les palettes claire et sombre définissent une toile, une surface, une surface élevée, du texte principal et secondaire, une couleur d'accent et les états succès, attention et erreur. Le texte normal vise WCAG AA (4,5:1), les composants et grands textes 3:1. Les tests calculent les ratios sRGB.

La pile egui reste le repli garanti. Sur macOS, une police système locale peut être ajoutée seulement si le fichier est lisible et possède un en-tête SFNT valide ; aucune police Apple n'est redistribuée. Noto Emoji reste embarquée pour les pictogrammes configurables.

## Structure et états

À partir de 1 024 points, la navigation est latérale (184 points). Sous ce seuil elle devient une rangée défilable afin que la vue active et les actions globales restent atteignables à 400 × 300. Une page contient identité, navigation, en-tête, contenu puis état contextuel.

Les états communs sont : normal, survol, focus clavier, désactivé, progression, succès, indisponible et erreur. Les libellés restent du texte accessible ; les formes dessinées ne portent jamais seules une information.

## Mouvement et mesures

Les transitions durent 160 ms (plage autorisée 120–180 ms), ne bouclent pas et ne demandent aucun repaint après stabilisation. Les workers peuvent demander un repaint borné tant qu'ils travaillent.

Le harness sérialise la taille, le thème, le budget de transition et l'état idle pour 400 × 300 et 1280 × 800. La qualification native mesure séparément : premier frame ≤ 2,5 s, 95e percentile d'une transition ≤ 16,7 ms et CPU idle ≤ 1 %.
