// Qt Test 自动化回归（v3.3.0 #11 收尾）：
// 覆盖 C ABI 契约（D43）与客户端启动前必须成立的前置条件，可无界面跑 CI。
#include <QtTest>

#include "acs_abi.h"

class TestAbi : public QObject
{
    Q_OBJECT

private slots:
    void initTestCase()
    {
        const QString err = Acs::init();
        QVERIFY2(err.isEmpty(), qPrintable(err));
    }

    void abiVersionMatches()
    {
        QCOMPARE(Acs::abiVersion(), Acs::ABI_EXPECTED);
    }

    void productVersionReadable()
    {
        QVERIFY(!Acs::version().isEmpty());
        QVERIFY(Acs::version().contains(QLatin1Char('.')));
    }

    void infoMethod()
    {
        QString err, code;
        const QJsonObject info = Acs::call(QStringLiteral("info"), {}, &err, &code);
        QVERIFY2(err.isEmpty(), qPrintable(Acs::errorText(err, code)));
        QCOMPARE(info.value(QStringLiteral("abi_version")).toInt(),
                 static_cast<int>(Acs::ABI_EXPECTED));
        QCOMPARE(info.value(QStringLiteral("version")).toString(), Acs::version());
    }

    void unknownMethodIsStructuredError()
    {
        QString err, code;
        Acs::call(QStringLiteral("no_such_method"), {}, &err, &code);
        QCOMPARE(code, QStringLiteral("unknown_method"));
        QVERIFY(!err.isEmpty());
    }

    void missingRequiredField()
    {
        QString err, code;
        Acs::call(QStringLiteral("login"), {}, &err, &code);
        QCOMPARE(code, QStringLiteral("invalid_json"));
    }

    void legalDocLocalMethod()
    {
        QString err, code;
        const QJsonObject doc = Acs::call(QStringLiteral("legal_doc"),
                                          {{QStringLiteral("doc"), QStringLiteral("privacy")}},
                                          &err, &code);
        QVERIFY2(err.isEmpty(), qPrintable(Acs::errorText(err, code)));
        QVERIFY(doc.value(QStringLiteral("html")).toString().contains(QStringLiteral("<html")));
    }

    void utf8RoundTrip()
    {
        // 中文/€ 必须 UTF-8 原样往返（LPUTF8Str 编组，见 D43）
        QString err, code;
        Acs::call(QStringLiteral("legal_doc"),
                  {{QStringLiteral("doc"), QStringLiteral("不存在的文档")}}, &err, &code);
        QCOMPARE(code, QStringLiteral("invalid_json"));
        QVERIFY(err.contains(QStringLiteral("不存在的文档")));
    }

    void brandHasNoSecrets()
    {
        QString err, code;
        const QJsonObject brand = Acs::call(QStringLiteral("brand"), {}, &err, &code);
        QVERIFY2(err.isEmpty(), qPrintable(Acs::errorText(err, code)));
        QVERIFY(brand.contains(QStringLiteral("source")));
        QVERIFY(!brand.contains(QStringLiteral("password")));
        QVERIFY(!brand.contains(QStringLiteral("seckey")));
    }

    void centerHistoryListable()
    {
        QString err, code;
        const QJsonObject centers = Acs::call(QStringLiteral("centers"), {}, &err, &code);
        QVERIFY2(err.isEmpty(), qPrintable(Acs::errorText(err, code)));
        QVERIFY(centers.contains(QStringLiteral("items")));
        QVERIFY(centers.value(QStringLiteral("items")).isArray());
    }
};

QTEST_MAIN(TestAbi)
#include "test_abi.moc"
