# Handleiding Open Pointcloud Studio

Deze handleiding beschrijft de native Rust-versie die na [v0.9.5](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/releases/tag/v0.9.5) uitkomt, met de gedeelde OpenAEC-vormgeving van titelbalk, tabbladen, panelen, instellingen en het menu **Bestand**. De afbeeldingen tonen die vormgeving in de Nederlandse interface en het thema Licht, met twee gegenereerde voorbeeldscènes: een huis met tuin (`building.laz`) en twee kamers met elk een scannerstation en een panoramafoto (`rooms.e57`), beschreven in [TEST_DATA](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/native/TEST_DATA.md#generated-demo-scenes). De ontwikkelversie waarmee ze zijn gemaakt, toont midden in de statusbalk nog v0.9.5. Installeer het pakket voor Windows, macOS of Linux via [Releases](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/releases/latest). De applicatie draait zonder browser of webview.

## 1. Oriëntatie

![Het venster met de titelbalk, de tabbladen Bestand en Start, de Start-ribbon, de tabbladen 3D-model, Begane grond, Doorsnede A en Voortuin boven de scène, de projectbrowser met building.laz, een gegenereerd huis met tuin in de 3D-weergave en de eigenschappen van de scan](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/wiki/images/overzicht.png)

Bovenaan staat de titelbalk; rechts daarin staan snelknoppen voor importeren, een scanmap openen, exporteren, ongedaan maken en opnieuw uitvoeren, en **Instellingen**. Daaronder staan de tabbladen **Bestand** en **Start**. De **Start**-ribbon bevat de groepen BESTAND, NAVIGATIE, SELECTIE, SNEDEBOX, OPPERVLAK, TEKENING, METEN, ANNOTATIE, INDEX en EXPORT. Op een smaller venster klappen hele groepen van rechts in tot een knop met een uitklapmenu.

**Bestand** opent een menu met **Nieuw**, directe acties (**Puntenwolk importeren**, **Scanmap openen**, **Tekening (DXF/DWG)…**, **3D BAG-gebouwen…** en vijf exports), **Alle exportopties…**, **Werkruimte**, **Extensies** en **Over**. Onderaan staan **Instellingen…**, **Terug naar model** en **Afsluiten**.

Links staat de **Projectbrowser** met de groepen SCANS, KLASSEN, VIEWS, BLADEN en BCF. Onder de ribbon loopt een strook met tabbladen: het 3D-model, geopende views en tekeningen, bladen en foto's. Rechts toont **Eigenschappen** informatie over de actieve scan en de instellingen van het gereedschap dat in gebruik is. De statusbalk onderaan toont onder meer het aantal punten, de selectie, de zoom en het geheugengebruik.

De knop **Weergave** onder de **kijkkubus**, rechtsboven in de 3D-weergave, opent de weergaveopties: CAMERA, KLEUR, DETAIL, **Eye-dome** en **Scanposities**. Zo heeft de ribbon maar één hoofdtabblad.

![Het paneel Weergave onder de kijkkubus, geopend met de rijen CAMERA, KLEUR met Hoogte gekozen, DETAIL met Puntgrootte en Punten in beeld, Eye-dome en Scanposities; het huis en de tuin zijn op hoogte gekleurd, van blauw laag naar rood hoog](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/wiki/images/weergave.png)

## 2. Scan openen en navigeren

1. Klik **Openen** in de groep BESTAND of kies **Bestand → Puntenwolk importeren**. Kies een LAS-, LAZ-, E57-, PLY-, PCD-, PTX-, PTS- of tekstbestand. Een map met scans (**Scanmap openen**) of een `.rcp`-projectbestand kan ook worden geopend of op het venster worden gesleept.
2. Bij een grote scan verschijnt eerst een voorbeeld. De strook boven het model laat zien hoe de rest wordt ingelezen en geïndexeerd, met een knop om te annuleren. De oorspronkelijke scan blijft ongewijzigd.
3. Gebruik het **muiswiel** om bij de aanwijzer te zoomen, de **middelste of rechter muisknop** om te pannen en **Alt + linkersleep** of **Shift + middelsleep** om te draaien. Met **F** of **Alles tonen** breng je het hele model in beeld. Met **Pan** in de groep NAVIGATIE pan je ook met de linkermuisknop.
4. Klik op een vlak of hoek van de **kijkkubus** voor een vaste richting. Het knopje eronder wisselt tussen **PERSP** (perspectief) en **ORTHO** (orthografisch).
5. Onder **Weergave** kies je bij KLEUR **RGB**, **Hoogte**, **Intensiteit** of **Klasse**, en stel je bij DETAIL **Puntgrootte** en **Punten in beeld** in. De rij CAMERA heeft **Iso**, **Boven**, **Voor**, **Rechts**, **Alles tonen**, **Selectie tonen** en **Stations passen**.

Klik links in de projectbrowser op een scan om die actief te maken; de eigenschappen van die scan verschijnen rechts. Voor scans vanaf één miljoen punten maakt de app een herbruikbare octree-index op schijf. De 3D-weergave leest vervolgens detail voor de huidige camera, tot het aantal **Punten in beeld**. Dat maakt ook scans van vele gigabytes hanteerbaar, afhankelijk van opslag, RAM en GPU. Een ongewijzigd bestand gebruikt bij een volgende opening de bestaande index.

## 3. Punten selecteren en meten

Klik op een punt om het te selecteren of sleep een rechthoek rond een gebied. De groep SELECTIE heeft **Selecteer** (punt kiezen), **Vak selecteren**, **Selectie wissen**, **Geselecteerde punten verwijderen** en **Selectie exporteren**. **Rechtsklik** in de 3D-weergave opent het contextmenu. **Escape** verlaat het actieve gereedschap en heft ook de selectie op. **Delete** verbergt geselecteerde punten; **Ctrl+Z** en **Ctrl+Y** maken de laatste verwijdering of fotokleuring ongedaan of herhalen die.

Kies **Afstand** in de groep METEN en klik twee punten in de 3D-weergave; de meting eindigt bij het tweede punt. **Oppervlakte** meet een vlak: klik de hoekpunten en rond af met **Enter** of met een klik op het eerste punt. **Backspace** verwijdert het laatste meetpunt. De resultaten verschijnen onder **Meten** in **Eigenschappen**, waar ook dezelfde twee knoppen staan.

![Een selectie van 765.312 punten op het dak van het huis, oranje gemarkeerd, en een verticale afstandsmeting van 5,600 m langs de hoek van de voorgevel; onder Meten in de eigenschappen staan de lengte, de horizontale lengte en het hoogteverschil](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/wiki/images/selectie.png)

## 4. Snedebox en 2D-tekening

Klik **Snedebox** en verschuif de vlakken in de 3D-weergave of vul de XYZ-grenzen in onder **Snedebox** in **Eigenschappen**. De box kan ook worden gedraaid. Zo beperk je het zichtbare gebied en de invoer voor een mesh of tekening. Voor een plattegrond leg je het bovenvlak op de gewenste snijhoogte; voor een verticale doorsnede kies je het betreffende zijvlak.

![Een snedebox rond het huis met de handgrepen op de vlakken; buiten de box zijn de rest van de tuin, de bomen en de straat weggesneden, en de box is bewaard als de view Voortuin, die als tabblad open is](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/wiki/images/snedebox.png)

Klik **2D snede** in de groep TEKENING om een **Snedetekening** van de snedebox te maken. Kies het **Aanzicht**, stel de **Plakdikte (m)** in, controleer het **Voorbeeld** en exporteer als **DXF** of **DWG** met **Tekening exporteren…**. Een **Gevulde snede** geeft wanden, kolommen en vloeren als gevulde gebieden weer. **Rechte CAD-lijnen** voegt bovendien gesloten, bewerkbare lijnen langs de snede toe, op een eigen laag, met een instelbare **Lijnafwijking (m)** en **Minimumlijn (m)**. Een voorbeeld van een plattegrond: bovenvlak op 1,10 m, plakdikte 0,10 m; de tekening gebruikt dan de punten tussen 1,00 en 1,10 m. De tekening opent ook als tabblad in de app.

Een plattegrond, aanzicht of doorsnede van het hele model maak je met **2D-plattegrond / aanzicht / doorsnede maken…**, een pictogram op de band van VIEWS in de projectbrowser.

![De doorsnede Doorsnede A dwars door het huis als eigen tabblad: het profiel van gevels en zadeldak op het maaiveld, met rechte CAD-lijnen in blauw; de eigenschappen tonen de tekeningweergave, de aantekeningen en de lagen van de tekening](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/wiki/images/doorsnede-2d.png)

## 5. Mesh en oppervlakken

Klik **Mesh** in de groep OPPERVLAK en kies de methode die bij je scan past: **Gesloten mesh** voor een gesloten gebied, **Terreinmesh** voor maaiveld, **3D-oppervlak** voor een open oppervlak of **Vlakken** om onder meer wanden, vloeren en cilindrische elementen te herkennen. De knoppen **Gesloten**, **Terrein**, **3D mesh** en **Vlakken** naast **Mesh** openen dezelfde kaart met die methode gekozen. Beperk de berekening zo nodig met de snedebox, stel de opties in of kies **Aanbevolen gebruiken**, en klik **Uitvoeren**.

Een taak loopt door als je de kaart sluit: de knop **Mesh** blijft dan gemarkeerd en opent de kaart weer op de stap **Uitvoeren**. De strook boven het model toont de stap van een gesloten mesh of een vlakkenherkenning; **Eigenschappen** toont de voortgang van een terreinmesh of 3D-oppervlak. Controleer het resultaat en sla een mesh op met **Mesh exporteren…** als OBJ, PLY, STL, DXF, DWG of IFC. **Oppervlak textureren…** schrijft een mesh met kleuren als OBJ met een MTL-bestand en PNG-textuurpagina's.

![De kaart Puntenwolk meshen op de stap Methode met de vier methoden Gesloten mesh, Terreinmesh, 3D-oppervlak en Vlakken, en daaronder waar het op werkt: de actieve scan rooms.e57, de snedebox en de selectie](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/wiki/images/mesh-methoden.png)

De resolutie, puntdichtheid en ruis bepalen de kwaliteit en rekentijd. **Vlakken** levert herkende geometrie; het is een andere uitkomst dan een doorlopend driehoeksmesh. [De uitgebreide gids](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/docs/guide.md#mesh-pointcloud) beschrijft de grenzen van de methoden.

## 6. E57-foto's, stations en weergaven

Een E57 kan scannerstations en foto's bevatten. Zet **Scanposities** aan onder **Weergave** en klik op een stationbol om de scan vanaf die plaats te bekijken; klik op een ander station om erheen te gaan. Met **W/A/S/D** en **Q/E** kun je in die stand bewegen; **Escape** brengt je terug. Panoramafoto's en foto's langs een traject verschijnen op hun opnamestandpunt.

Een E57-foto opent in een eigen tabblad; de camera van het 3D-model blijft bewaard. Met **Naast 3D tonen** zie je de foto en het model tegelijk. Versleep de scheidingslijn, of kies **Onder elkaar** of **Naast elkaar**. De modelhelft heeft een eigen draaibeweging, pan, zoom en kijkkubus. De indeling blijft na een herstart bewaard. Met **Kleur uit foto's** in **Eigenschappen** kun je punten op basis van de bijbehorende E57-foto's inkleuren; controleer vooraf de **Grootste afstand (m)**.

![De panoramafoto van scannerstation 2 in een eigen tabblad, Station 2, rechts van het tabblad 3D-model, met Foto 2 van 2, de schuif Foto en de knop Naast 3D tonen rechtsboven; de foto kijkt door de open deur naar de andere kamer, en de eigenschappen tonen onder Scanposities 2 stations en onder Foto's 2 panorama's](https://raw.githubusercontent.com/OpenAEC-Foundation/open-pointcloud-studio/main/docs/wiki/images/stations-foto.png)

Typ in de projectbrowser onder VIEWS een naam en kies **View opslaan**. Een view bewaart de camera, de snedebox en de kleurmodus, en opent later in een eigen tabblad. Onder **BLADEN** stel je views en tekeningen samen op een blad, dat je als PDF exporteert. Opgeslagen views met notities kunnen als **BCF** worden geëxporteerd, via **Bestand → Alle exportopties… → Views als BCF…**.

## 7. Export en sneltoetsen

Het menu **Bestand** heeft vijf directe exports van de actieve scan: **Volledige resolutie exporteren…**, **Geselecteerde punten exporteren…**, **Snedebox exporteren…**, **Snedetekening exporteren…** en **Elk N-de punt exporteren…**. **Bestand → Alle exportopties…** toont alle exports, ook meshes, vlakken en BCF. Kies daar het **Formaat**: onder andere LAS, LAZ, E57, PLY, XYZ, PTS en CSV. Met **Behoud 1 op** schrijft **Elk N-de punt** bijvoorbeeld elk tiende punt. Kijk vóór een export welke scan actief is en welke punten zijn geselecteerd of verborgen. De bronbestanden worden niet overschreven door verbergen, uitdunnen of verplaatsen in de geopende sessie.

| Invoer | Actie |
| --- | --- |
| Muiswiel | Zoom bij de aanwijzer |
| Middel- of rechtersleep | Pan |
| Alt + linkersleep of Shift + middelsleep | Draai de camera |
| F | Toon het hele model |
| Linkerklik of linkersleep | Selecteer punt of rechthoek |
| Delete | Verberg geselecteerde punten |
| Enter | Rond een oppervlaktemeting af; plaats een notitie |
| Backspace | Verwijder het laatste meetpunt |
| Escape | Sluit een open paneel, dialoog of kaart; verlaat anders het gereedschap en hef de selectie op |
| Ctrl+Z / Ctrl+Y | Ongedaan maken / opnieuw |
| Ctrl+Tab | Volgend tabblad |

Op macOS gebruik je **Command** in plaats van **Ctrl**, behalve voor het wisselen van tabbladen.

Meer details staan in de [uitgebreide gebruikersgids](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/docs/guide.md), de [README](https://github.com/OpenAEC-Foundation/open-pointcloud-studio#readme) en de [API/MCP-documentatie](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/blob/main/native/API.md).
