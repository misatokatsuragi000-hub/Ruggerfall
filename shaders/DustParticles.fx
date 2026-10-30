/*
    DustParticles.fx - 3D World-Aware Atmospheric Dust Motes
    Integrato con DFU_CameraPose per rotazione, traslazione e occlusione 3D reali.
*/

#include "ReShade.fxh"

// ----------------------------------------------------------------------------------
// Parametri Interfaccia Utente (UI)
// ----------------------------------------------------------------------------------

uniform float DustDensity <
    ui_type = "slider";
    ui_min = 0.0; ui_max = 2.5; ui_step = 0.05;
    ui_category = "Aspetto Polvere";
    ui_label = "Densità / Visibilità";
    ui_tooltip = "Controlla l'intensità complessiva delle particelle.";
> = 1.0;

uniform float DustSize <
    ui_type = "slider";
    ui_min = 0.4; ui_max = 2.0; ui_step = 0.05;
    ui_category = "Aspetto Polvere";
    ui_label = "Dimensione Granelli";
    ui_tooltip = "Fattore di grandezza di ciascuna particella.";
> = 1.0;

uniform float FloatSpeed <
    ui_type = "slider";
    ui_min = 0.05; ui_max = 1.5; ui_step = 0.02;
    ui_category = "Movimento";
    ui_label = "Velocità Fluttuazione";
    ui_tooltip = "Velocità di deriva naturale della polvere nell'aria.";
> = 0.30;

uniform float TwinkleSpeed <
    ui_type = "slider";
    ui_min = 0.5; ui_max = 6.0; ui_step = 0.1;
    ui_category = "Aspetto Polvere";
    ui_label = "Velocità Luccichio";
    ui_tooltip = "Velocità con cui i granelli brillano nel tempo.";
> = 2.2;

uniform float ForegroundBokeh <
    ui_type = "slider";
    ui_min = 0.0; ui_max = 1.5; ui_step = 0.05;
    ui_category = "Aspetto Polvere";
    ui_label = "Intensità Bokeh (Primo Piano)";
    ui_tooltip = "Visibilità dei granelli sfocati molto vicini alla telecamera.";
> = 0.65;

uniform float2 WindDirection <
    ui_type = "slider";
    ui_min = -1.0; ui_max = 1.0; ui_step = 0.05;
    ui_category = "Movimento";
    ui_label = "Direzione Vento Mondo (X, Z)";
    ui_tooltip = "Direzione del vento nello spazio del mondo di gioco.";
> = float2(0.20, -0.15);

uniform float3 DustColor <
    ui_type = "color";
    ui_category = "Aspetto Polvere";
    ui_label = "Colore Polvere";
    ui_tooltip = "Tonalità dei granelli (bianco caldo / paglierino).";
> = float3(1.0, 0.95, 0.85);

// ---------------------------------------------------------------- Camera / 3D Sync
uniform bool UseCameraPose <
    ui_category = "Integrazione DFU_CameraPose";
    ui_label = "Usa DFU_CameraPose";
    ui_tooltip = "Attiva l'ancoraggio 3D reale al mondo tramite DFU_CameraPose.";
> = true;

uniform float CP_FOV <
    ui_type = "slider"; ui_min = 40.0; ui_max = 120.0; ui_step = 1.0;
    ui_category = "Integrazione DFU_CameraPose";
    ui_label = "FOV verticale (gradi)";
    ui_tooltip = "DEVE corrispondere al FOV impostato in DFU_CameraPose e in Daggerfall Unity.";
> = 60.0;

uniform float CP_FarPlane <
    ui_type = "slider"; ui_min = 10.0; ui_max = 2000.0; ui_step = 1.0;
    ui_category = "Integrazione DFU_CameraPose";
    ui_label = "Far plane (unita')";
    ui_tooltip = "DEVE corrispondere al Far Plane impostato in DFU_CameraPose.";
> = 1000.0;

uniform bool CP_FlipDepthY <
    ui_category = "Integrazione DFU_CameraPose";
    ui_label = "Capovolgi depth (verticale)";
    ui_tooltip = "Usa la stessa impostazione scelta in DFU_CameraPose.";
> = true;

uniform float Timer < source = "timer"; >;

// ----------------------------------------------------------------------------------
// Lettura Posa di Mondo condivisa da DFU_CameraPose
// ----------------------------------------------------------------------------------

namespace DFUPose
{
    texture2D tWorld { Width = 3; Height = 1; Format = RGBA32F; };
}
sampler sDFUWorld { Texture = DFUPose::tWorld; MinFilter = POINT; MagFilter = POINT; AddressU = CLAMP; AddressV = CLAMP; };

float4 FetchPose(int x)
{
    return tex2Dlod(sDFUWorld, float4((x + 0.5) / 3.0, 0.5, 0.0, 0.0));
}

float3 QuatRotate(float4 q, float3 v)
{
    float3 u = q.xyz;
    return v + 2.0 * q.w * cross(u, v) + 2.0 * cross(u, cross(u, v));
}

float GetLinearDepth(float2 uv)
{
    if (CP_FlipDepthY) uv.y = 1.0 - uv.y;
    return ReShade::GetLinearizedDepth(uv) * CP_FarPlane;
}

// ----------------------------------------------------------------------------------
// Hash e Rumore 3D
// ----------------------------------------------------------------------------------

float3 Hash33(float3 p)
{
    p = frac(p * float3(443.897, 441.423, 437.195));
    p += dot(p, p.yzx + 19.19);
    return frac((p.xxy + p.yxx) * p.zyx);
}

// Rendering di uno strato di polvere volumetrico ancorato nello spazio 3D
float RenderDustLayer3D(float3 rayDirCam, float3 camPosWorld, float4 camRotWorld,
                        float targetDist, float sceneDepth,
                        float gridDensity, float speedMult, float baseSize,
                        float layerSeed, bool isBokeh)
{
    // Occlusione con la geometria: se il muro è più vicino dello strato, sfuma
    float occFade = smoothstep(0.0, targetDist * 0.25, sceneDepth - targetDist);
    if (occFade <= 0.001)
        return 0.0;

    // Punto nello spazio telecamera alla distanza dello strato
    float3 pCam = rayDirCam * targetDist;

    // Conversione in coordinate di Mondo (se abilitato DFU_CameraPose)
    float3 pWorld = (UseCameraPose) ? (QuatRotate(camRotWorld, pCam) + camPosWorld) : pCam;

    // Deriva continua del vento nello spazio 3D
    float time = (Timer * 0.001) * FloatSpeed * speedMult;
    float3 worldWind = float3(WindDirection.x, -0.25, WindDirection.y) * time;
    float3 samplePos = (pWorld - worldWind) * gridDensity;

    float3 id = floor(samplePos);
    float3 gv = frac(samplePos);

    // Cerca nelle 8 celle più vicine (2x2x2) per garantire continuità senza bordi
    int3 stepDir = (gv > 0.5) ? int3(1, 1, 1) : int3(-1, -1, -1);

    float layerIntensity = 0.0;

    [unroll]
    for (int z = 0; z <= 1; ++z)
    {
        for (int y = 0; y <= 1; ++y)
        {
            for (int x = 0; x <= 1; ++x)
            {
                float3 cellOffset = float3(x ? stepDir.x : 0,
                                           y ? stepDir.y : 0,
                                           z ? stepDir.z : 0);
                float3 cellID = id + cellOffset;

                float3 rnd = Hash33(cellID + float3(layerSeed, layerSeed * 1.618, layerSeed * 2.414));

                // Moto browniano (micro-turbolenza d'aria sui 3 assi)
                float3 sway = float3(
                    sin(time * 2.0 + rnd.y * 6.28) * 0.12,
                    cos(time * 1.7 + rnd.x * 6.28) * 0.12,
                    sin(time * 1.4 + rnd.z * 6.28) * 0.12
                );

                float3 particlePos = cellOffset + 0.15 + 0.70 * rnd + sway;
                float d = distance(gv, particlePos);

                // Raggio e luccichio
                float radius = baseSize * DustSize * (0.6 + rnd.x * 0.7);
                float twinkle = lerp(0.35, 1.0, sin(time * TwinkleSpeed + rnd.y * 31.41) * 0.5 + 0.5);

                float p = 0.0;
                if (isBokeh)
                {
                    p = smoothstep(radius, radius * 0.35, d) * 0.65 +
                        smoothstep(radius * 1.6, 0.0, d) * 0.35;
                }
                else
                {
                    p = smoothstep(radius, 0.0, d) +
                        smoothstep(radius * 2.8, 0.0, d) * 0.35;
                }

                layerIntensity += p * twinkle;
            }
        }
    }

    return layerIntensity * occFade;
}

// ----------------------------------------------------------------------------------
// Pixel Shader
// ----------------------------------------------------------------------------------

float4 PS_DustParticles(float4 pos : SV_Position, float2 uv : TEXCOORD) : SV_Target
{
    float3 sceneColor = tex2D(ReShade::BackBuffer, uv).rgb;

    if (DustDensity <= 0.001)
        return float4(sceneColor, 1.0);

    // Lettura della posa globale della telecamera
    float4 qw = FetchPose(0);
    if (dot(qw, qw) < 0.5) qw = float4(0.0, 0.0, 0.0, 1.0); // Fallback identità
    float3 tw = FetchPose(1).xyz;                           // Posizione nel mondo

    // Costruzione del raggio di vista nello spazio telecamera (basato su FOV e Aspect Ratio)
    float th = tan(radians(CP_FOV * 0.5));
    float asp = BUFFER_ASPECT_RATIO;
    float3 rayDir = normalize(float3((uv.x * 2.0 - 1.0) * th * asp, (1.0 - uv.y * 2.0) * th, 1.0));

    // Lettura della profondità reale per occludere la polvere dietro i muri
    float sceneDepth = GetLinearDepth(uv);

    // 1. Primo piano (Bokeh): granelli grandi e sfocati a ~0.8 unità dalla camera
    float fg = RenderDustLayer3D(rayDir, tw, qw, 0.8, sceneDepth, 2.2, 1.3, 0.110, 89.12, true) * ForegroundBokeh * 0.75;

    // 2. Piano medio: granelli nitidi a ~2.5 unità dalla camera
    float mid = RenderDustLayer3D(rayDir, tw, qw, 2.5, sceneDepth, 4.5, 0.8, 0.045, 45.67, false) * 0.85;

    // 3. Sfondo: polvere fine e lontana a ~6.0 unità dalla camera
    float bg = RenderDustLayer3D(rayDir, tw, qw, 6.0, sceneDepth, 9.0, 0.4, 0.022, 11.23, false) * 0.45;

    float totalDust = (fg + mid + bg) * DustDensity;

    // Miscelazione luminosa (screen-blend) per mantenere la naturalezza dei colori
    float3 dustLight = DustColor * totalDust;
    float3 finalColor = 1.0 - (1.0 - sceneColor) * (1.0 - saturate(dustLight));

    return float4(finalColor, 1.0);
}

// ----------------------------------------------------------------------------------
// Tecnica
// ----------------------------------------------------------------------------------

technique DustParticles
<
    ui_tooltip = "Polvere volumetrica 3D integrata con DFU_CameraPose. Posiziona questo shader SOTTO DFU_CameraPose nell'elenco.";
>
{
    pass
    {
        VertexShader = PostProcessVS;
        PixelShader  = PS_DustParticles;
    }
}
