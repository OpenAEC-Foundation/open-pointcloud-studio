# Handleiding Open Pointcloud Studio

Deze handleiding hoort bij de [native Rust-versie v0.9.5](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/releases/tag/v0.9.5). Installeer het pakket voor Windows, macOS of Linux via [Releases](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/releases/latest). De applicatie draait zonder browser of webview.

## 1. Oriëntatie

![Start-ribbon, projectbrowser, 3D-weergave en eigenschappen](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/screenshots/start-ribbon-v095.png)

De **Start**-ribbon bevat de hoofdgereedschappen. **Bestand** opent acties voor bestanden en instellingen. Links staat de **Projectbrowser** met scans, opgeslagen weergaven, tekeningen en bladen. Middenin staan de geopende tabbladen en het 3D-model. Rechts toont **Eigenschappen** informatie over de actieve scan en het actieve gereedschap. De statusbalk onderaan toont onder meer het aantal punten, de selectie, de zoom en het geheugengebruik.

Weergaveopties zijn gegroepeerd onder de **kijkkubus**, rechtsboven in de 3D-weergave. Zo blijft er maar één hoofdtabblad in de ribbon.

![Weergaveopties onder de kijkkubus](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/screenshots/view-controls-v095.png)

## 2. Scan openen en navigeren

1. Klik **Openen** of gebruik **Bestand → Puntenwolk importeren**. Kies een LAS-, LAZ-, E57-, PLY-, PCD-, PTX-, PTS- of tekstbestand. Een map met scans of een `.rcp`-projectbestand kan ook worden geopend of op het venster worden gesleept.
2. Bij een grote scan verschijnt eerst een voorbeeld. De voortgang boven het model laat zien hoe de rest wordt ingelezen en geïndexeerd. De oorspronkelijke scan blijft ongewijzigd.
3. Gebruik het **muiswiel** om bij de aanwijzer te zoomen, de **middelste of rechter muisknop** om te pannen en **Alt + linkersleep** of **Shift + middelsleep** om te draaien. Met **F** breng je het hele model in beeld.
4. Klik op de **kijkkubus** voor een vaste richting. Onder **Weergave** kun je onder meer kleurmodus, puntgrootte en puntbudget instellen.

Voor scans vanaf ongeveer één miljoen punten maakt de app een herbruikbare octree-index op schijf. De 3D-weergave leest vervolgens detail voor de huidige camera tot het gekozen puntbudget. Dat maakt ook scans van vele gigabytes hanteerbaar, afhankelijk van opslag, RAM en GPU.

## 3. Punten selecteren en meten

Klik op een punt om het te selecteren of sleep een rechthoek rond een gebied. In de ribbon staan ook selectiegereedschappen. **Rechtsklik** in de 3D-weergave opent het contextmenu. **Escape** verlaat het actieve gereedschap; daarna kan een volgende Escape de selectie opheffen. **Delete** verbergt geselecteerde punten; **Ctrl+Z** en **Ctrl+Y** maken de laatste bewerking ongedaan of herhalen die.

Voor een afstand kies je **Afstand** en klik je opeenvolgende punten in de 3D-weergave. **Enter** rondt de meting af en **Backspace** verwijdert het laatste meetpunt. **Oppervlakte** meet een vlak dat je met hoekpunten afbakent. Resultaten verschijnen in **Eigenschappen**.

![Puntselectie in de 3D-weergave](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/promo/stills/02-selection.png)

## 4. Snedebox en 2D-tekening

Klik **Snedebox** en verschuif de vlakken in de 3D-weergave of vul de XYZ-grenzen in bij **Eigenschappen**. Zo beperk je het zichtbare gebied en de invoer voor een mesh of tekening. Een vlakke laag levert een plattegrond; een verticale snede levert een doorsnede of aanzicht.

![Een snedebox rond de scan](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/promo/stills/03-section.png)

Klik **2D snede** om een tekening uit de snedebox te maken. Stel de richting en dikte van de snijlaag in, controleer het voorbeeld en exporteer als **DXF** of **DWG**. Een gevulde snede kan wanden, kolommen en vloeren als gevulde gebieden weergeven. De tekening opent ook als tabblad in de app.

![Een 2D-doorsnede als eigen tabblad](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/promo/stills/07-section-2d.png)

## 5. Mesh en oppervlakken

Klik **Mesh** en kies de methode die bij je scan past: **Gesloten mesh** voor een gesloten gebied, **Terreinmesh** voor maaiveld, **3D-oppervlak** voor een open oppervlak of **Vlakken** om onder meer wanden, vloeren en cilindrische elementen te herkennen. Beperk de berekening zo nodig met de snedebox. Stel de resolutie in, controleer het resultaat en sla een mesh op als **OBJ**, **PLY** of **STL**.

![De methoden in de mesh-wizard](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/promo/stills/04-mesher.png)

De resolutie, puntdichtheid en ruis bepalen de kwaliteit en rekentijd. **Vlakken** levert herkende geometrie; het is een andere uitkomst dan een doorlopend driehoeksmesh. [De uitgebreide gids](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/docs/guide.md#mesh-pointcloud) beschrijft de grenzen van de methoden.

## 6. E57-foto's, stations en weergaven

Een E57 kan scannerstations en foto's bevatten. Zet de stations zichtbaar via **Weergave** en klik op een stationbol om de scan vanaf die plaats te bekijken. Met **W/A/S/D** en **Q/E** kun je in die stand bewegen; **Escape** brengt je terug. Panoramafoto's en foto's langs een traject verschijnen op hun opnamestandpunt. Met **Kleur uit foto's** kun je punten op basis van de bijbehorende E57-foto's inkleuren; controleer vooraf de maximale afstand.

![Scannerstations en panoramabeeld](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/promo/stills/08-panorama.png)

In de projectbrowser kun je een **Weergave opslaan**. Die bewaart camera en zichtbaarheid en kan later in een eigen tabblad worden geopend. Onder **BLADEN** stel je weergaven en tekeningen samen. Opgeslagen weergaven met notities kunnen als **BCF** worden geëxporteerd.

## 7. Export en sneltoetsen

Gebruik **Bestand → Exporteren** om de actieve scan, een selectie of de inhoud van de snedebox uit te voeren. Puntwolkformaten zijn onder andere LAS, LAZ, E57, PLY, XYZ, PTS en CSV. Bij export kun je bijvoorbeeld elk tiende punt laten schrijven. De bronbestanden worden niet overschreven door verbergen, uitdunnen of verplaatsen in de geopende sessie.

| Invoer | Actie |
| --- | --- |
| Muiswiel | Zoom bij de aanwijzer |
| Middel- of rechtersleep | Pan |
| Alt + linkersleep of Shift + middelsleep | Draai de camera |
| F | Toon het hele model |
| Linkerklik of linkersleep | Selecteer punt of rechthoek |
| Enter / Backspace | Rond meting af / verwijder laatste meetpunt |
| Escape | Verlaat gereedschap; hef daarna selectie op |
| Ctrl+Z / Ctrl+Y | Ongedaan maken / opnieuw |
| Ctrl+Tab | Volgend tabblad |

Meer details staan in de [uitgebreide gebruikersgids](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/docs/guide.md), de [README](https://github.com/OpenAEC-Foundation/open-pointcloud-studio#readme) en de [API/MCP-documentatie](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/native/API.md).
